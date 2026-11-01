"""The actual Torch RMSNorm consumer, native allocation census and complete refusal."""
from __future__ import annotations

import torch
from cozy_runtime.author import Context, denoise_loop
from minimax_h3.adaln_pruned import _AdaLNPrunedOutputTable
from torch.utils._python_dispatch import TorchDispatchMode

from . import Probe, Report, actual, app, report, settle

WIDTH = 5376


class Census(TorchDispatchMode):
    def __init__(self, vt, device):
        super().__init__()
        self.vt, self.device, self.calls = vt, device, []
        self.inference = torch.is_inference_mode_enabled()
        self.grad = torch.is_grad_enabled()

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        active = self.vt.split_context(self.device)['reason'] == 'leaf_active'
        before = self.vt.stats(self.device) if active else None
        with torch.inference_mode(self.inference), torch.set_grad_enabled(self.grad):
            result = func(*args, **(kwargs or {}))
        if active:
            after = self.vt.stats(self.device)
            outputs = result if isinstance(result, tuple) else (result,)
            self.calls.append({
                'op': str(func), 'allocations': after['allocations'] - before['allocations'],
                'requested_live_delta': after['allocated_bytes'] - before['allocated_bytes'],
                'class_live_delta': after['allocated_capacity'] - before['allocated_capacity'],
                'outputs': [{'shape': list(x.shape), 'dtype': str(x.dtype),
                             'requested_bytes': x.numel() * x.element_size()}
                            for x in outputs if isinstance(x, torch.Tensor)],
            })
        return result


def construction(on, dtype):
    root = torch.nn.Module()
    root.norm_out = _AdaLNPrunedOutputTable(
        torch.nn.RMSNorm(WIDTH, eps=1e-6, device=on, dtype=dtype),
        hidden_size=WIDTH, timestep_count=2)
    # Only norm_out.norm is in scope; the package's table/modulation math is unchanged.
    from cozy_runtime.internal.execution import cuts
    assert cuts.install(root) == 1
    return root


@app.entrypoint(accelerator=True)
def norm(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime.internal import prebuilt, verify

    request, vt, provenance = actual(payload, ctx)
    device, on = ctx.device.index or 0, request.memory.device
    torch.set_num_threads(1)
    settle(vt, device)
    initial = vt.stats(device)
    result = {'provenance': provenance, 'initial': initial, 'cases': [], 'keys': [],
              'scope': 'exact pinned Torch RMSNorm whole operation; no artificial split menu'}

    def run(result):
        root = x = marker = expected = step = minimum = None
        try:
            with torch.no_grad():
                for dtype, rows in ((torch.bfloat16, 32768), (torch.float16, 513),
                                    (torch.float32, 513)):
                    root = construction(on, dtype)
                    module = root.norm_out.norm
                    module.weight.copy_(torch.randn((WIDTH,), dtype=dtype,
                        generator=torch.Generator().manual_seed(payload.seed + 3)).to(on))
                    x = torch.randn((1, rows, WIDTH), dtype=dtype,
                                    generator=torch.Generator().manual_seed(payload.seed)).to(on)
                    marker = torch.zeros(1, dtype=torch.int64, device=on)
                    expected = torch.nn.functional.rms_norm(
                        x.view(rows, WIDTH), (WIDTH,), module.weight, module.eps).view_as(x).cpu()
                    settle(vt, device)
                    held = {}

                    def step(i, marker, values, *, module=module, held=held):
                        del i, marker
                        before = len(request.working_phases)
                        prior = {key: row['selections'] for key, row in request.working_splits.items()}
                        census = Census(vt, device)
                        with census:
                            output = module(values)
                        assert not output.requires_grad
                        host = output.cpu()
                        changed = [row for key, row in request.working_splits.items()
                                   if row['selections'] > prior.get(key, 0)
                                   and key.startswith('runtime.split.rms_norm/1:')]
                        assert len(changed) == 1, changed
                        record = dict(changed[0])
                        held.update(record=record, output=host, census=census.calls, phase_at=before)
                        del output

                    denoise_loop(step, range(1), state=(marker,), inputs=(x,),
                                 key=('rms-whole', str(dtype), rows))
                    record = held['record']
                    phase = request.working_phases[-1]
                    child = next(row for row in phase['split_observations'] if row['key'] == record['key'])
                    census = held['census']
                    native = [row for row in census if row['op'] == 'aten._fused_rms_norm.default']
                    error = verify.error(torch, held['output'], expected)
                    case = {'case': f'whole/{dtype}/{rows}', 'record': record, 'phase': phase,
                            'census': census, 'relative_l2': error,
                            'bit_exact': torch.equal(held['output'], expected)}
                    result['cases'].append(case)
                    result['keys'].append(record['key'])
                    assert child['published'] and record['native_calls'] == 1
                    assert len(native) == 1 and sum(row['allocations'] for row in census) == 2
                    observed = native[0]
                    expected_sizes = [rows * WIDTH * x.element_size(), rows * 4]
                    assert [row['requested_bytes'] for row in observed['outputs']] == expected_sizes
                    assert observed['allocations'] == 2
                    assert observed['requested_live_delta'] == sum(expected_sizes)
                    assert observed['outputs'][1]['dtype'] == 'torch.float32'
                    assert error <= prebuilt.BUDGET
                    held.clear()
                    expected = None
                    settle(vt, device)
                    key, allocations = record['key'], record['allocations']
                    prior_sample = vt.split_select(device, key, [('whole', allocations)],
                                                    preferred='whole', stream=int(torch.cuda.current_stream(device).cuda_stream))
                    assert prior_sample['basis'] == 'measured', prior_sample
                    refusal = {}

                    def minimum(i, marker, values, *, module=module, refusal=refusal,
                                key=key, allocations=allocations):
                        del i, marker
                        settle(vt, device)
                        selected = vt.split_select(device, key, [('whole', allocations)],
                                                   preferred='whole', stream=int(torch.cuda.current_stream(device).cuda_stream))
                        assert selected['required_bytes'] > 0
                        before = vt.stats(device)
                        cap = (before['mapped_bytes'] + request.memory.native.stats()['resident_bytes']
                               + selected['required_bytes'] - 1)
                        vt.set_limit(device, cap)
                        failure = None
                        try:
                            module(values)
                        except torch.OutOfMemoryError as refusal_error:
                            failure = str(refusal_error)
                            assert getattr(refusal_error, 'varena_no_retry', False)
                        finally:
                            vt.set_limit(device, None)
                        after = vt.stats(device)
                        refusal.update(error=failure, cap=cap, selection=selected,
                                       before=before, after=after)
                        assert failure and before['allocations'] == after['allocations']

                    denoise_loop(minimum, range(1), state=(marker,), inputs=(x,),
                                 key=('rms-minimum', str(dtype), rows))
                    after_sample = vt.split_select(device, key, [('whole', allocations)],
                                                    preferred='whole', stream=int(torch.cuda.current_stream(device).cuda_stream))
                    assert after_sample['basis'] == 'measured'
                    assert after_sample['measured_backing_peak_bytes'] == prior_sample['measured_backing_peak_bytes']
                    result['cases'].append({'case': f'minimum/{dtype}/{rows}', **refusal,
                                            'before_sample': prior_sample, 'after_sample': after_sample,
                                            'phase': request.working_phases[-1]})
                    step = minimum = None
                    root = module = x = marker = None
                    settle(vt, device)
        finally:
            vt.set_limit(device, None)
            step = minimum = None
            root = module = x = marker = expected = None
            settle(vt, device)
            result['final'] = vt.stats(device)
            result['keys'] = sorted(set(result['keys']) | request.working_phase_keys)
            for field in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes',
                          'pending_bytes', 'pools', 'pools_bytes', 'event_handles'):
                assert result['final'][field] == initial[field], (field, initial, result['final'])
    return report(result, run)
