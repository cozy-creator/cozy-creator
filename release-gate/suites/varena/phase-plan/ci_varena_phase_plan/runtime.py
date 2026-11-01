"""Actual file-backed NativeLinear blocks adopt only established region representations."""
from __future__ import annotations

import json
import tempfile
from pathlib import Path

import torch
from cozy_runtime.author import Context, Cut, denoise_loop, split
from torch import nn

from . import Probe, Report, actual, app, settle

ROWS, WIDTH, CHANNELS, LAYERS = 8193, 4096, 1024, 8


class Layer(nn.Module):
    def __init__(self):
        from cozy_runtime.internal.encoding.leaves import Int8ConvRotNativeLeaf

        super().__init__()
        original = nn.Linear(WIDTH, CHANNELS, bias=False, device='meta', dtype=torch.bfloat16)
        self.proj = Int8ConvRotNativeLeaf().leaf(
            torch, {'weight': torch.empty((CHANNELS, WIDTH), dtype=torch.int8, device='meta'),
                    'scale': torch.empty(CHANNELS, dtype=torch.float32, device='meta')},
            original, torch.bfloat16)

    def forward(self, values):
        return split(self.proj, values, Cut(1))


class Network(nn.Module):
    def __init__(self):
        super().__init__()
        self.layers = nn.ModuleList(Layer() for _ in range(LAYERS))

    def forward(self, values):
        from cozy_runtime.internal import prebuilt, verify

        rows = []
        for layer in self.layers:
            output = layer(values)
            actual = output.cpu()
            # Every input row is the same, but each real stored layer differs.
            row = actual[:, :1].clone()
            assert verify.error(torch, actual, row.expand_as(actual)) <= prebuilt.BUDGET
            rows.append(row)
            del output, actual, row
        return rows


class Recipe:
    def __init__(self):
        self.revision = 1
        self.fail = False
        self.escaped = None

    @property
    def identity(self):
        return f'fixture.phase-plan/recipe/{self.revision}'

    def overlay(self, block, parts):
        return None

    def __call__(self, block, parts):
        if self.fail and block == 'layers.0':
            data = next(roles['value'] for name, roles in parts.items() if name.endswith('.data'))
            data.fill_(77)
            self.escaped = data.view(-1)[:16]
            raise RuntimeError('real phase-plan fill failure after an escaped write')
        return False


def checkpoint(directory):
    import tensorfs

    writer = tensorfs.Writer(Path(directory) / 'phase-plan.cozytensors')
    component = writer.component('int8')
    for index in range(LAYERS):
        component.put(f'layers.{index}.proj.data', torch.full((CHANNELS, WIDTH), index + 1,
                                                            dtype=torch.int8))
        component.put(f'layers.{index}.proj.scale', torch.full((CHANNELS,), 0.001,
                                                             dtype=torch.float32))
    return writer.commit()


def load(memory, source, recipe):
    from cozy_runtime.internal import residency

    network = Network()
    vars(network)['_cozy_weights'] = residency.Declared(
        residency.Weights(source, 'int8'), blocks=[f'layers.{i}' for i in range(LAYERS)], fill=recipe)
    return network, memory.load(network, 'ram')


def allocations(native, device, chunk):
    tail = ROWS % chunk or chunk
    queries = [native.int8_plan(device, False, count, CHANNELS, WIDTH, CHANNELS)
               for count in (chunk, tail)]
    assert all(row['status'] == 0 for row in queries)
    quantized = max((n for n, info in zip((chunk, tail), queries, strict=True)
                     if info['kind'] == 0), default=0)
    result = [ROWS * CHANNELS * 2]
    if quantized:
        result += [quantized * WIDTH, quantized * 4]
    workspace = max(q['workspace_bytes'] for q in queries)
    if workspace:
        result.append(workspace)
    return result, queries


@app.entrypoint(accelerator=True)
def runtime(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime import _kernels
    from cozy_runtime.internal import int8_linear, prebuilt, verify

    request, vt, provenance = actual(payload, ctx)
    memory, device = request.memory, ctx.device.index or 0
    assert int8_linear.native(torch, device) is not None
    assert int8_linear.library(device).family == 'mma.sync'
    torch.set_num_threads(1)
    settle(vt, device)
    initial = vt.stats(device)
    cases = []
    with torch.inference_mode(), tempfile.TemporaryDirectory(prefix='runtime-phase-plan-') as directory:
        source = checkpoint(directory)
        recipe = Recipe()
        network, model = load(memory, source, recipe)
        request.plan([network])
        values = torch.ones((1, ROWS, WIDTH), dtype=torch.bfloat16, device=memory.device)
        marker = torch.zeros(1, dtype=torch.int64, device=memory.device)
        results = {}

        def pass_once(chunk=None, *, network=network, model=model, values=values,
                      marker=marker, results=results):
            results.clear()
            before = {'store': request.moved, 'host': request.baked_bytes,
                      'weights': memory.native.stats(), 'phase_events': len(request.phase_changes)}
            prior = {key: row['selections'] for key, row in request.working_splits.items()}
            key = f'fixture.runtime-phase-plan/capacity/{chunk}'

            def step(index, marker, values, *, chunk=chunk):
                del index, marker
                settle(vt, device)
                if chunk is not None:
                    menu, _ = allocations(_kernels._gemm, device, chunk)
                    selected = vt.split_select(device, key, [('target', menu)], preferred='target',
                                               stream=int(torch.cuda.current_stream(device).cuda_stream))
                    weight_bytes = sum(block.region.allocated_bytes for block in model.groups())
                    cap = vt.stats(device)['mapped_bytes'] + weight_bytes + selected['required_bytes']
                    vt.set_limit(device, cap)
                    results['managed_limit'] = cap
                results['outputs'] = network(values)

            try:
                denoise_loop(step, range(1), state=(marker,), inputs=(values,),
                             key=('actual-file-native-linear-phase', ROWS, WIDTH, CHANNELS))
            finally:
                vt.set_limit(device, None)
                if chunk is not None:
                    vt.phase_forget(device, key)
            current = [json.loads(json.dumps(row)) for key, row in request.working_splits.items()
                       if row['selections'] > prior.get(key, 0)]
            assert sum(row['selections'] - prior.get(row['key'], 0) for row in current) == LAYERS
            samples = request.working_phases[-1]['split_observations']
            assert len(samples) == LAYERS and all(s['published'] and not s['pending'] for s in samples)
            if chunk is not None:
                assert all(s['choice_id'].startswith(f'rows:{chunk}/') for s in samples), samples
            record = {'before': before, 'after': {'store': request.moved, 'host': request.baked_bytes,
                                                 'weights': memory.native.stats()},
                      'selections': current, 'phase': request.working_phases[-1],
                      'phase_events': request.phase_changes[before['phase_events']:],
                      'managed_limit': results.get('managed_limit')}
            return results.pop('outputs'), record

        try:
            reference, calibration = pass_once()
            assert any(row.get('region_plan', {}).get('reason') == 'representation_regions_incomplete'
                       for row in calibration['phase_events'])
            assert all(row['selection']['phase'] is None for row in calibration['selections'])
            regions = [block.region for block in model.groups()]
            assert len(regions) == LAYERS and all(region is not None for region in regions)
            pointers = [region.ptr for region in regions]
            cases.append({'case': 'first_pass_is_unplanned', **calibration})
            for chunk in (ROWS, (ROWS + 1) // 2):
                memory.trim()
                settle(vt, device)
                assert memory.native.stats()['resident_bytes'] == 0
                output, cold = pass_once(chunk)
                for got, want in zip(output, reference, strict=True):
                    assert verify.error(torch, got, want) <= prebuilt.BUDGET
                assert all(row['row_chunk'] == chunk for row in cold['selections'])
                assert all(row['selection']['preserves_phase_weights'] for row in cold['selections'])
                assert all(region.resident for region in regions)
                assert [block.region.ptr for block in model.groups()] == pointers
                output, warm = pass_once(chunk)
                assert warm['after']['store'] == warm['before']['store']
                assert warm['after']['host'] == warm['before']['host']
                assert warm['after']['weights']['fills'] == warm['before']['weights']['fills']
                for got, want in zip(output, reference, strict=True):
                    assert verify.error(torch, got, want) <= prebuilt.BUDGET
                cases.append({'case': 'actual_native_consumer_capacity', 'row_chunk': chunk,
                              'cold': cold, 'warm': warm})
                del output
            assert memory._phase_plan is not None
            recipe.revision += 1
            output, changed = pass_once()
            assert any(row.get('region_plan', {}).get('status') == 'unplanned'
                       for row in changed['phase_events'])
            assert all(row['selection']['phase'] is None for row in changed['selections'])
            for got, want in zip(output, reference, strict=True):
                assert verify.error(torch, got, want) <= prebuilt.BUDGET
            del output
            output, rebound = pass_once()
            assert memory._phase_plan is not None
            del output
            memory.trim()
            settle(vt, device)
            old_pointer = next(iter(model.groups())).region.ptr
            recipe.fail = True
            try:
                pass_once()
            except RuntimeError as error:
                assert 'real phase-plan fill failure' in str(error)
            else:
                raise AssertionError('the real Fill failure did not run')
            assert memory._phase_plan is None and recipe.escaped is not None
            assert torch.equal(recipe.escaped.cpu(), torch.full((16,), 77, dtype=torch.int8))
            recipe.fail = False
            output, recovered = pass_once()
            assert next(iter(model.groups())).region.ptr != old_pointer
            assert torch.equal(recipe.escaped.cpu(), torch.full((16,), 77, dtype=torch.int8))
            for got, want in zip(output, reference, strict=True):
                assert verify.error(torch, got, want) <= prebuilt.BUDGET
            cases.append({'case': 'identity_and_failed_fill_retire_plan', 'changed': changed,
                          'rebound': rebound, 'recovered': recovered, 'escaped_alias_preserved': True})
            del output, regions
        finally:
            vt.set_limit(device, None)
            recipe.escaped = None
            owned_keys = sorted(model.working_phase_keys)
            model.close()
            assert memory._phase_plan is None
            assert owned_keys and not any(vt.phase_forget(device, key) for key in owned_keys)
            del network, model, values, marker, recipe, pass_once, results
            memory.trim()
            settle(vt, device)
    final = vt.stats(device)
    assert final['allocated_bytes'] == initial['allocated_bytes'], (initial, final)
    assert final['pending_bytes'] == 0 and final['pools'] == initial['pools']
    assert memory.native.stats()['resident_bytes'] == 0
    return Report(True, json.dumps({'passed': True, 'provenance': provenance, 'cases': cases,
                                   'initial': initial, 'final': final,
                                   'model_keys_retired_at_close': owned_keys,
                                   'request_owned_keys': sorted(request.working_phase_keys)}, sort_keys=True))
