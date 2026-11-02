"""One ordinary file-backed NativeLinear forward, with no inferred closure or fake source."""
from __future__ import annotations

import gc
import sys
import tempfile
from pathlib import Path

import tensorfs
import torch
from cozy_runtime.author import Context
from torch import nn

from . import Probe, Report, actual, app, report, settle

ROWS, WIDTH, CHANNELS = 8195, 4096, 1024


class Layer(nn.Module):
    def __init__(self):
        from cozy_runtime.internal.encoding.leaves import Int8ConvRotNativeLeaf

        super().__init__()
        original = nn.Linear(WIDTH, CHANNELS, bias=False, device='meta', dtype=torch.bfloat16)
        self.proj = Int8ConvRotNativeLeaf().leaf(
            torch, {'weight': torch.empty((CHANNELS, WIDTH), dtype=torch.int8, device='meta'),
                    'scale': torch.empty(CHANNELS, dtype=torch.float32, device='meta')},
            original, torch.bfloat16)


class Network(nn.Module):
    def __init__(self):
        super().__init__()
        self.layers = nn.ModuleList(Layer() for _ in range(2))


def checkpoint(directory, seed=13819):
    writer = tensorfs.Writer(Path(directory) / 'committed-linear.cozytensors')
    component = writer.component('int8')
    generator = torch.Generator().manual_seed(seed)
    for index in range(2):
        component.put(f'layers.{index}.proj.data', torch.randint(
            -32, 33, (CHANNELS, WIDTH), dtype=torch.int8, generator=generator))
        component.put(f'layers.{index}.proj.scale', torch.full((CHANNELS,), .001,
                                                             dtype=torch.float32))
    return writer.commit()


def load(memory, source):
    from cozy_runtime.internal import residency

    network = Network()
    vars(network)['_cozy_weights'] = residency.Declared(
        residency.Weights(source, 'int8'), blocks=['layers.0.proj', 'layers.1.proj'])
    return network, memory.load(network, 'ram')


def compare(left, right):
    from cozy_runtime.internal import prebuilt, verify

    error = verify.error(torch, left, right)
    return {'relative_l2': error, 'bit_exact': bool(torch.equal(left, right)),
            'limit': prebuilt.BUDGET, 'passed': error <= prebuilt.BUDGET}


def invoke(phase):
    """Instrument actual Python forward invocations, not a manually incremented test stub."""
    code = phase.module.forward.__func__.__code__
    counts = {'forward_calls': 0}
    previous = sys.getprofile()
    assert previous is None

    def count(frame, event, arg):
        if (event == 'call' and frame.f_code is code
                and frame.f_locals.get('self') is phase.module):
            counts['forward_calls'] += 1

    sys.setprofile(count)
    try:
        value = phase()
    finally:
        sys.setprofile(previous)
    assert counts['forward_calls'] == 1, counts
    return value, counts


@app.entrypoint(accelerator=True)
def consumer(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime.internal import int8_linear
    from cozy_runtime.internal.execution.committed_linear import inventory

    request, vt, provenance = actual(payload, ctx)
    memory, device = request.memory, ctx.device.index or 0
    assert int8_linear.native(torch, device) is not None
    assert int8_linear.library(device).family == 'mma.sync'
    torch.set_num_threads(1)
    settle(vt, device)
    initial = vt.stats(device)
    initial_weights = memory.native.stats()
    result = {'provenance': provenance, 'initial': initial, 'initial_weights': initial_weights,
              'cases': [], 'keys': [], 'scope': 'explicit single raw-source linear phase only'}

    def run(result):
        model = source = values = x = phase = value = reference = wrong = None
        retired_regions = []
        try:
            with torch.inference_mode(), tempfile.TemporaryDirectory(
                    prefix='committed-linear-') as directory:
                source = checkpoint(directory)
                network, model = load(memory, source)
                leaf = network.layers[0].proj
                request.plan([network])
                values = torch.randn((1, ROWS, WIDTH), dtype=torch.bfloat16,
                                     generator=torch.Generator().manual_seed(13823)).to(memory.device)
                settle(vt, device)
                before = {'working': vt.stats(device), 'weights': memory.native.stats(),
                          'reads': request.moved, 'pins': [block.count for block in model.groups()]}
                phase = inventory(model, leaf, values, next_block='layers.1.proj')
                after = {'working': vt.stats(device), 'weights': memory.native.stats(),
                         'reads': request.moved, 'pins': [block.count for block in model.groups()]}
                result['cases'].append({'case': 'metadata-only-inventory', 'before': before,
                                        'after': after, 'site': phase.site})
                assert after == before
                assert all(block.native_use is None and block.data is None for block in model.groups())
                assert all(block.region is None or not block.region.resident for block in model.groups())

                first_key = None
                for label, x in (('cold-source-unknown-compute', values),
                                 ('cold-source-known-compute', values),
                                 ('new-reservations-known-compute', values),
                                 ('changed-shape-unknown', values[:, :-2])):
                    # Release Python views, retaining reservation identities; native reclaim
                    # makes idle source backing cold without assuming a victim order.
                    if label == 'new-reservations-known-compute':
                        retired_regions = [block.region for block in model.groups()
                                           if block.region is not None]
                        old_addresses = [region.ptr for region in retired_regions]
                        for block in model.groups():
                            memory._discard(block, forget=True)
                    else:
                        old_addresses = []
                    memory._discard(model.blocks['layers.1.proj'])
                    gc.collect()
                    memory.native.reclaim(memory.native.stats()['reclaimable_bytes'])
                    settle(vt, device)
                    phase = inventory(model, leaf, x, next_block='layers.1.proj')
                    start = {'started': request.prefetch_started, 'completed': request.prefetch_completed,
                             'declined': request.prefetch_declined, 'trace': len(request.trace),
                             'working': vt.stats(device), 'phases': len(request.working_phases)}
                    value, calls = invoke(phase)
                    record = dict(request.working_splits[phase.site])
                    assert record['invocations'] == 1 and record['phase']['published'], record
                    key = record['key']
                    result['keys'] += [phase.site, key]
                    if first_key is None:
                        first_key = key
                    with model.passing('reference', executing=True):
                        reference = leaf(x)
                    torch.cuda.synchronize(device)
                    error = compare(value.cpu(), reference.cpu())
                    assert error['passed'], error
                    trace = request.trace[start['trace']:]
                    prefetched = [row for row in trace if row['speculative']]
                    case = {'case': label, **calls, 'record': record, 'comparison': error,
                            'prefetch_started': request.prefetch_started - start['started'],
                            'prefetch_completed': request.prefetch_completed - start['completed'],
                            'prefetch_declined': request.prefetch_declined - start['declined'],
                            'speculative_reads': prefetched,
                            'successful_sample': vt.phase_stats(device, key)}
                    result['cases'].append(case)
                    if label in ('cold-source-known-compute', 'new-reservations-known-compute'):
                        assert key == first_key
                        if label == 'cold-source-known-compute':
                            assert record['selection']['plan_generation'] > 0
                        assert case['prefetch_started'] == case['prefetch_completed'] == 1, case
                        assert sum(row['bytes'] for row in prefetched) == model.blocks['layers.1.proj'].storage_bytes
                    else:
                        assert not prefetched and case['prefetch_started'] == 0, case
                        assert case['prefetch_declined'] >= 1, case
                        if label == 'changed-shape-unknown':
                            assert key != first_key
                    if old_addresses:
                        new_addresses = [block.region.ptr for block in model.groups()
                                         if block.region is not None]
                        case.update(old_reserved_addresses=old_addresses,
                                    new_reserved_addresses=new_addresses)
                        assert new_addresses and not set(old_addresses) & set(new_addresses)
                    retired_regions.clear()
                    value = reference = None
                    settle(vt, device)
                # Retain this fixture's two real sources while applying a managed cap from
                # the native half-menu query. The producer still chooses from its full menu;
                # no Python row selector or memory estimate participates.
                with model.passing('tight-current-choice', executing=True):
                    blocks = model.groups()
                    entered = []
                    try:
                        for block in blocks:
                            memory._enter(block)
                            entered.append(block)
                        settle(vt, device)
                        phase = inventory(model, leaf, values, next_block='layers.1.proj')
                        target = phase.choices[1]
                        probe_key = phase.site + '/tight-bound'
                        request.memory.remember_working_phase(probe_key, (model,))
                        result['keys'].append(probe_key)
                        chosen = vt.split_select(device, probe_key, [target], preferred=target[0],
                                                 stream=int(torch.cuda.current_stream(device).cuda_stream))
                        assert chosen['required_bytes'] > 0
                        cap = (vt.stats(device)['mapped_bytes']
                               + memory.native.stats()['resident_bytes'] + chosen['required_bytes'])
                        vt.set_limit(device, cap)
                        try:
                            value, calls = invoke(phase)
                        finally:
                            vt.set_limit(device, None)
                        record = dict(request.working_splits[phase.site])
                        reference = leaf(values)
                        torch.cuda.synchronize(device)
                        error = compare(value.cpu(), reference.cpu())
                        result['keys'] += [phase.site, record['key']]
                        case = {'case': 'tight-current-choice', **calls, 'native_target': chosen,
                                'cap': cap, 'record': record, 'comparison': error}
                        result['cases'].append(case)
                        assert record['row_chunk'] < ROWS and record['key'] != first_key, case
                        assert record['phase']['published']
                        assert record['phase']['successful_passes'] == 1 and error['passed']
                        value = reference = None
                    finally:
                        for block in reversed(entered):
                            memory._exit(block)
                settle(vt, device)
                # A same-shaped real CUDA buffer of unrelated source identity must not execute.
                phase = inventory(model, leaf, values)
                with model.passing('wrong-binding', executing=True):
                    block = model.blocks['layers.0.proj']
                    memory._enter(block)
                    try:
                        actual_data = leaf.data
                        wrong = actual_data.clone()
                        leaf.data = wrong
                        before = vt.stats(device)
                        old_phases = len(request.working_phases)
                        failure = None
                        try:
                            value = phase()
                        except RuntimeError as error:
                            failure = str(error)
                        after = vt.stats(device)
                        case = {'case': 'same-layout-wrong-binding', 'error': failure,
                                'before': before, 'after': after,
                                'phase_delta': len(request.working_phases) - old_phases}
                        result['cases'].append(case)
                        assert failure and 'binding differs' in failure
                        assert after['allocations'] == before['allocations']
                        assert not case['phase_delta'] and value is None
                    finally:
                        leaf.data = actual_data
                        actual_data = wrong = None
                        memory._exit(block)
                # Native minimum refusal is induced using its actual required backing.
                # The current source remains pinned, so no idle weight can fund the gap.
                memory._discard(model.blocks['layers.1.proj'], forget=True)
                memory.native.reclaim(memory.native.stats()['reclaimable_bytes'])
                with model.passing('minimum-refusal', executing=True):
                    block = model.blocks['layers.0.proj']
                    memory._enter(block)
                    try:
                        settle(vt, device)
                        phase = inventory(model, leaf, values)
                        probe_key = phase.site + '/minimum-bound'
                        request.memory.remember_working_phase(probe_key, (model,))
                        result['keys'].append(probe_key)
                        minimum = phase.choices[-1]
                        chosen = vt.split_select(device, probe_key, [minimum], preferred=minimum[0],
                                                 stream=int(torch.cuda.current_stream(device).cuda_stream))
                        assert chosen['required_bytes'] > 0, chosen
                        before = vt.stats(device)
                        cap = (before['mapped_bytes'] + memory.native.stats()['resident_bytes']
                               + chosen['required_bytes'] - 1)
                        sample = vt.phase_stats(device, first_key)
                        before_phases = len(request.working_phases)
                        failure = None
                        vt.set_limit(device, cap)
                        try:
                            value = phase()
                        except torch.OutOfMemoryError as error:
                            failure = str(error)
                            assert getattr(error, 'varena_no_retry', False)
                        finally:
                            vt.set_limit(device, None)
                        after = vt.stats(device)
                        case = {'case': 'actual-native-minimum-refusal', 'error': failure,
                                'native_minimum': chosen, 'cap': cap, 'before': before, 'after': after,
                                'phase_delta': len(request.working_phases) - before_phases}
                        result['cases'].append(case)
                        assert failure and value is None and not case['phase_delta'], case
                        assert after['allocations'] == before['allocations']
                        assert vt.phase_stats(device, first_key) == sample
                    finally:
                        memory._exit(block)
                # The real checkpoint is closed, then a forced cold demand attempts to read it.
                # This is an actual source-owner error, not an injected OOM or fake fill callback.
                sample = vt.phase_stats(device, first_key)
                phase = inventory(model, leaf, values, next_block='layers.1.proj')
                memory._discard(model.blocks['layers.0.proj'], forget=True)
                memory.native.reclaim(memory.native.stats()['reclaimable_bytes'])
                source.close()
                before_phases = len(request.working_phases)
                failure = None
                try:
                    value = phase()
                except (RuntimeError, ValueError, tensorfs.Error) as error:
                    failure = f'{type(error).__name__}: {error}'
                after_sample = vt.phase_stats(device, first_key)
                case = {'case': 'cold-closed-source', 'error': failure,
                        'phase_delta': len(request.working_phases) - before_phases,
                        'before_sample': sample, 'after_sample': after_sample}
                result['cases'].append(case)
                assert failure and not case['phase_delta'] and value is None, case
                assert sample == after_sample
                # Same actual geometry and kernels, different checkpoint content. Replacing
                # the source owns a fresh current key, independently of earlier observations.
                first_source = phase.source
                phase = None
                model.close()
                model = None
                source = None
                changed = Path(directory) / 'changed'
                changed.mkdir()
                source = checkpoint(changed, seed=13820)
                network, model = load(memory, source)
                leaf = network.layers[0].proj
                request.plan([network])
                phase = inventory(model, leaf, values, next_block='layers.1.proj')
                assert phase.source != first_source
                before_started = request.prefetch_started
                value, calls = invoke(phase)
                record = dict(request.working_splits[phase.site])
                result['keys'] += [phase.site, record['key']]
                with model.passing('changed-source-reference', executing=True):
                    reference = leaf(values)
                torch.cuda.synchronize(device)
                error = compare(value.cpu(), reference.cpu())
                case = {'case': 'changed-source-unknown', **calls, 'old_source': first_source,
                        'new_source': phase.source, 'record': record, 'comparison': error,
                        'prefetch_started': request.prefetch_started - before_started}
                result['cases'].append(case)
                assert record['key'] != first_key and record['phase']['published']
                assert case['prefetch_started'] == 0 and error['passed']

        finally:
            vt.set_limit(device, None)
            phase = value = reference = wrong = values = x = None
            retired_regions.clear()
            if model is not None:
                model.close()
            model = None
            if source is not None:
                source.close()
            source = None
            settle(vt, device)
            memory.native.reclaim(memory.native.stats()['reclaimable_bytes'])
            settle(vt, device)
            result['final'] = vt.stats(device)
            result['final_weights'] = memory.native.stats()
            assert result['final']['allocated_bytes'] == initial['allocated_bytes']
            assert result['final']['mapped_bytes'] == initial['mapped_bytes']
            assert result['final']['pending_bytes'] == result['final']['pools_bytes'] == 0
            assert result['final_weights']['resident_bytes'] == initial_weights['resident_bytes']
            assert result['final_weights']['pinned_bytes'] == initial_weights['pinned_bytes']

    return report(result, run)
