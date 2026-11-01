"""Actual Runtime demand retention and graph/alias ownership, without mocked owners."""
from __future__ import annotations

import gc
import hashlib
import json
import os
import tempfile
from importlib.metadata import version
from pathlib import Path

import msgspec
import tensorfs
import torch
from cozy_runtime.author import App, Context, denoise_loop
from torch import nn

app = App()
ELEMENTS = 1 << 20
BYTES = ELEMENTS * 4
LAYERS = 5


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    residency_sha256: str
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


class Add(nn.Module):
    def __init__(self):
        super().__init__()
        self.weight = nn.Parameter(torch.empty(ELEMENTS, device='meta', dtype=torch.float32),
                                   requires_grad=False)

    def forward(self, value):
        return value + self.weight


class Network(nn.Module):
    def __init__(self):
        super().__init__()
        self.bias = nn.Parameter(torch.empty(1, device='meta', dtype=torch.float32),
                                 requires_grad=False)
        self.layers = nn.Sequential(*(Add() for _ in range(LAYERS)))

    def forward(self, value):
        # The common scalar must stay pinned across every paged child.
        return self.layers(value) + self.bias


class Tied(nn.Module):
    def __init__(self):
        super().__init__()
        self.a, self.b = Add(), Add()
        self.b.weight = self.a.weight

    def forward(self, value):
        assert self.a.weight is self.b.weight
        return self.b(self.a(value))


class Scaling:
    """Actual mutable source recipe; an escaped failed Fill remains an ordinary native Pin."""
    def __init__(self):
        self.scale = 1
        self.fail = False
        self.escaped = None

    @property
    def identity(self):
        return f'fixture.retention.scale/{self.scale}'

    def overlay(self, block, keys):
        return None

    def __call__(self, block, parts):
        for roles in parts.values():
            value = roles['value']
            if self.fail and block == 'layers.0':
                value.fill_(91)
                self.escaped = value[:16]
                raise RuntimeError('fixture fill failed after an actual partial write')
            value.mul_(self.scale)
        return self.scale != 1


def settle(vt, device):
    gc.collect()
    torch.cuda.synchronize(device)
    vt.trim(device)
    torch.cuda.synchronize(device)
    vt.trim(device)


def expected(scale=1):
    return torch.full((ELEMENTS,), scale * (1 + LAYERS * (LAYERS + 1) // 2),
                      device='cpu', dtype=torch.float32)


def figures(memory, request, vt, device):
    return {'weights': memory.native.stats(), 'working': vt.stats(device),
            'store_bytes': request.moved, 'host_bytes': request.baked_bytes}


def pass_once(network, inputs, request, vt, device, *, scale=1):
    """The ordinary public loop owns its actual phase and completion synchronization."""
    result = {}

    def step(index, state):
        del index
        result['output'] = network(state)

    before = figures(request.memory, request, vt, device)
    count = len(request.working_phases)
    denoise_loop(step, range(1), state=(inputs,), key=('native-retention-fixed-adds', ELEMENTS, scale))
    assert len(request.working_phases) == count + 1
    output = result.pop('output')
    torch.testing.assert_close(output.cpu(), expected(scale), rtol=0, atol=0)
    del output, step
    settle(vt, device)
    after = figures(request.memory, request, vt, device)
    assert after['weights']['reclaimed_by_reason']['streaming_limit'] == before['weights']['reclaimed_by_reason']['streaming_limit'], (before, after)
    return {'before': before, 'after': after, 'phase': request.working_phases[-1]}


def model(memory, checkpoint, component='plain', fill=None):
    from cozy_runtime.internal import residency

    network = Network()
    vars(network)['_cozy_weights'] = residency.Declared(
        residency.Weights(checkpoint, component),
        blocks=[f'layers.{index}' for index in range(LAYERS)], fill=fill,
    )
    return network, memory.load(network, 'ram')


def exercise(request, vt, device):
    memory = request.memory
    cases = []
    owners = []
    bindings = []
    graphs = []
    with tempfile.TemporaryDirectory(prefix='runtime-native-retention-') as directory:
        writer = tensorfs.Writer(Path(directory) / 'weights.cozytensors')
        for name in ('plain', 'scaled'):
            component = writer.component(name)
            component.put('bias', torch.ones(1, dtype=torch.float32))
            for index in range(LAYERS):
                component.put(f'layers.{index}.weight', torch.full((ELEMENTS,), index + 1.0))
        tied_component = writer.component('tied')
        shared = torch.full((ELEMENTS,), 2.0)
        tied_component.put('a.weight', shared)
        tied_component.put('b.weight', shared)
        del shared
        checkpoint = writer.commit()
        inputs = torch.zeros(ELEMENTS, dtype=torch.float32, device=memory.device)
        network, loaded = model(memory, checkpoint)
        owners.append(loaded)
        request.plan([network])
        try:
            # Measure this actual fixed program before choosing its two capacity settings.
            # No model working-memory estimate is substituted for the observed mapping peak.
            vt.reset_peak(device)
            calibration = pass_once(network, inputs, request, vt, device)
            working_peak = vt.stats(device)['mapped_peak']
            regions = [block.region for block in loaded.groups()]
            assert all(region is not None for region in regions)
            region_pointers = [region.ptr for region in regions]
            all_weights = sum(region.allocated_bytes for region in regions)
            common_bytes = loaded.common.region.allocated_bytes
            assert all_weights > common_bytes + 2 * BYTES
            memory.trim()
            settle(vt, device)
            assert memory.native.stats()['resident_bytes'] == 0

            roomy = working_peak + all_weights
            vt.set_limit(device, roomy)
            cold = pass_once(network, inputs, request, vt, device)
            warm = pass_once(network, inputs, request, vt, device)
            assert cold['after']['weights']['resident_bytes'] == all_weights, cold
            assert warm['after']['weights']['fills'] == cold['after']['weights']['fills'], warm
            assert warm['after']['store_bytes'] == cold['after']['store_bytes'], warm
            assert warm['after']['host_bytes'] == cold['after']['host_bytes'], warm
            assert [block.region.ptr for block in loaded.groups()] == region_pointers
            cases.append({'case': 'roomy_cold_then_warm', 'measured_working_peak': working_peak,
                          'actual_weight_backing': all_weights, 'managed_limit': roomy,
                          'calibration': calibration, 'cold': cold, 'warm': warm})

            memory.trim()
            settle(vt, device)
            tight = working_peak + common_bytes + 2 * BYTES
            assert tight < roomy
            vt.set_limit(device, tight)
            first = pass_once(network, inputs, request, vt, device)
            second = pass_once(network, inputs, request, vt, device)
            actual = second['after']['weights']
            assert actual['resident_bytes'] < all_weights, actual
            assert actual['reclaimed_by_reason']['allocator'] + actual['reclaimed_by_reason']['weight_allocation'] > first['before']['weights']['reclaimed_by_reason']['allocator'] + first['before']['weights']['reclaimed_by_reason']['weight_allocation'], (first, second)
            assert [block.region.ptr for block in loaded.groups()] == region_pointers
            cases.append({'case': 'tight_capacity_reclaims_on_demand', 'managed_limit': tight,
                          'first': first, 'second': second})
            vt.set_limit(device, None)

            # Retain through Runtime's real Binding, capture with Varena's actual pool,
            # then hold both an output alias and a weight alias through explicit pressure.
            binding = loaded.retain()
            bindings.append(binding)
            stream = torch.cuda.Stream(device=device)
            stream.wait_stream(torch.cuda.current_stream(device))
            graph = torch.cuda.CUDAGraph()
            graphs.append(graph)
            with binding.prepare(stream), torch.cuda.stream(stream):
                warm_output = network(inputs)
                stream.synchronize()
                del warm_output
                weight_alias = network.layers[0].weight[:16]
                weight_pointer = weight_alias.data_ptr()
                with vt.graph(graph, stream=stream, reserve_bytes=16 * BYTES):
                    captured = network(inputs)
            with binding.replay_use(stream):
                vt.replay(graph, stream=stream)
            stream.synchronize()
            torch.testing.assert_close(captured.cpu(), expected(), rtol=0, atol=0)
            output_alias = captured[::2]
            held = memory.native.stats()
            memory.native.reclaim(held['resident_bytes'])
            assert binding.resident() and memory.native.stats()['resident_bytes'] == held['resident_bytes']
            graph.reset()
            binding.close()
            del captured, graph, binding
            settle(vt, device)
            memory.reclaim()
            settle(vt, device)
            pinned = memory.native.stats()
            assert pinned['resident_bytes'] == pinned['pinned_bytes'] == BYTES, pinned
            assert weight_alias.data_ptr() == weight_pointer
            torch.testing.assert_close(weight_alias.cpu(), torch.ones(16), rtol=0, atol=0)
            torch.testing.assert_close(output_alias.cpu(), expected()[::2], rtol=0, atol=0)
            cases.append({'case': 'prepared_graph_and_escaped_aliases', 'before_pressure': held,
                          'after_owner_retirement': pinned, 'weight_pointer_preserved': True,
                          'graph_replayed': True, 'output_alias_preserved': True})
            del weight_alias, output_alias
            stream.synchronize()
            vt.retire_stream(stream, device=device)
            del stream
            memory.reclaim()
            settle(vt, device)
            assert memory.native.stats()['resident_bytes'] == 0
            loaded.close()
            owners.remove(loaded)
            del loaded, network, regions

            transform = Scaling()
            network, loaded = model(memory, checkpoint, 'scaled', transform)
            owners.append(loaded)
            request.plan([network])
            pass_once(network, inputs, request, vt, device)
            binding = loaded.retain()
            bindings.append(binding)
            with binding.prepare():
                old_alias = network.layers[0].weight[:16]
                old_pointer = old_alias.data_ptr()
            transform.scale = 2
            assert not binding.valid()
            binding.close()
            changed = pass_once(network, inputs, request, vt, device, scale=2)
            torch.testing.assert_close(old_alias.cpu(), torch.ones(16), rtol=0, atol=0)
            current = loaded.retain()
            bindings.append(current)
            with current.prepare():
                assert network.layers[0].weight.data_ptr() != old_pointer
            current.close()
            del current, binding, old_alias
            cases.append({'case': 'changed_fill_preserves_old_alias', 'changed': changed})

            memory.trim()
            transform.fail = True
            failed = False
            try:
                network(inputs)
            except RuntimeError as error:
                assert str(error) == 'fixture fill failed after an actual partial write', error
                failed = True
            assert failed and transform.escaped is not None
            assert loaded.blocks['layers.0'].region is None
            assert not memory.native.stats()['poisoned']
            torch.cuda.synchronize(device)
            torch.testing.assert_close(transform.escaped.cpu(), torch.full((16,), 91.0), rtol=0, atol=0)
            transform.fail = False
            repaired = pass_once(network, inputs, request, vt, device, scale=2)
            torch.testing.assert_close(transform.escaped.cpu(), torch.full((16,), 91.0), rtol=0, atol=0)
            transform.escaped = None
            cases.append({'case': 'failed_fill_then_healthy_demand', 'unpublished_region_retired': True,
                          'escaped_partial_bytes_preserved': True, 'healthy': repaired})
            from cozy_runtime.internal import residency

            tied = Tied()
            vars(tied)['_cozy_weights'] = residency.Declared(
                residency.Weights(checkpoint, 'tied'), blocks=['a', 'b'],
            )
            tied_owner = memory.load(tied, 'ram')
            owners.append(tied_owner)
            request.plan([tied])
            tied_output = tied(inputs)
            torch.testing.assert_close(tied_output.cpu(), torch.full((ELEMENTS,), 4.0), rtol=0, atol=0)
            assert tied_owner.common is not None
            cases.append({'case': 'tied_parameter_identity', 'common_owner': True,
                          'same_parameter_in_actual_forward': True, 'output_value': 4})
            del tied_output, tied
        finally:
            vt.set_limit(device, None)
            torch.cuda.synchronize(device)
            for graph_owner in reversed(graphs):
                graph_owner.reset()
            for binding_owner in reversed(bindings):
                binding_owner.close()
            for owner in reversed(owners):
                owner.close()
            request.plan([])
            checkpoint.close()
        del inputs, network, loaded
        memory.trim()
        settle(vt, device)
        assert not memory.models and memory.native.stats()['resident_bytes'] == 0
    return cases


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime.internal import int8_linear, residency
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    assert version('cozy-runtime') == payload.runtime_version
    assert digest(residency.__file__) == payload.residency_sha256
    assert version('tensorfs') == payload.tensorfs_version
    assert digest(_ext.__file__) == payload.extension_sha256
    assert digest(Path(tensorfs.__file__).resolve().parent / '_varena_torch.so') == payload.shim_sha256
    assert str(torch.__version__) == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    assert ctx.device.type == 'cuda' and torch.cuda.device_count() == 1
    device = ctx.device.index or 0
    uuid = str(torch.cuda.get_device_properties(device).uuid)
    assert uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    request = residency._REQUEST.get()
    assert isinstance(request, residency.Request) and request.id == ctx.request_id
    assert request.memory.requests == {request} and not request.memory.models and not request.idle
    assert request.memory.budget_bytes is None
    torch.set_num_threads(1)
    int8_linear.native(torch, device)
    settle(vt, device)
    initial = vt.stats(device)
    assert request.memory.native.stats()['resident_bytes'] == 0
    with torch.inference_mode():
        cases = exercise(request, vt, device)
    request.memory.reclaim()
    settle(vt, device)
    final = vt.stats(device)
    assert final['allocated_bytes'] == initial['allocated_bytes'], (initial, final)
    assert final['pending_bytes'] == 0 and final['pools'] == initial['pools']
    assert request.memory.native.stats()['resident_bytes'] == 0
    return Report(True, json.dumps({'passed': True, 'cases': cases, 'initial': initial,
        'final': final, 'pid': os.getpid(), 'request_id': ctx.request_id,
        'provenance': msgspec.to_builtins(payload),
        'scope': 'real Request/Memory/checkpoint hooks; native demand retention and existing Binding/graph/alias lifetime; no full-model or smaller-GPU claim'}, sort_keys=True))
