"""Actual installed-extension contract; GPU overlap cases require an owned GPU child."""
from __future__ import annotations
import argparse
import itertools
import json
import os
from tensorfs import _ext
from tensorfs.varena import torch as vt

parser = argparse.ArgumentParser()
parser.add_argument('--mode', choices=('host', 'gpu'), default='host')
parser.add_argument('--device', type=int, default=0)
parser.add_argument('--gpu-uuid')
args = parser.parse_args()
assert vt.PhaseOverlapError is _ext.PhaseOverlapError
assert issubclass(vt.PhaseOverlapError, RuntimeError)
report = {'passed': False, 'mode': args.mode, 'native_class': vt.PhaseOverlapError.__name__,
          'native_module': vt.PhaseOverlapError.__module__, 'public_alias_identity': True,
          'gpu_overlap_qualified': False, 'cases': []}


def ordinary_refusal(call, expected=RuntimeError):
    try:
        call()
    except vt.PhaseOverlapError as error:
        raise AssertionError('unrelated refusal was mislabeled as phase overlap') from error
    except expected as error:
        assert type(error) is expected, type(error)
        return {'type': type(error).__name__, 'message': str(error)}
    raise AssertionError('invalid operation unexpectedly entered')


if args.mode == 'host':
    # The actual installation check is metadata-only. Never initialize a CUDA
    # owner just to manufacture an active-phase state on a host-only lane.
    report['uninstalled'] = ordinary_refusal(_ext._varena_working_check)
    for mode in ('eager', 'preparation'):
        report['cases'].append({'requested_mode': mode, 'refusal': ordinary_refusal(
            lambda mode=mode: _ext._varena_phase_enter(0, 'host-uninstalled', mode=mode))})
    report['unknown_mode'] = ordinary_refusal(
        lambda: _ext._varena_phase_enter(0, 'host-unknown-mode', mode='not-a-phase'), ValueError)
    report.update(passed=True, gpu_executed=False,
                  scope='real native class/alias and pre-install refusal mapping only; no active-phase simulation')
else:
    assert args.gpu_uuid and args.gpu_uuid != 'UNBOUND'
    import torch
    assert not torch.cuda.is_initialized(), 'use a fresh isolated child'
    vt.install(args.device)
    torch.cuda.set_device(args.device)
    actual_uuid = str(torch.cuda.get_device_properties(args.device).uuid)
    normalize = lambda value: value.removeprefix('GPU-').replace('-', '').lower()
    assert normalize(actual_uuid) == normalize(args.gpu_uuid)
    stream = torch.cuda.Stream(device=args.device)
    event = torch.cuda.Event()
    keys = []
    try:
        with torch.inference_mode(), torch.cuda.stream(stream):
            for outer_mode, inner_mode in itertools.product(('eager', 'preparation'), repeat=2):
                key = f'phase-overlap:{os.getpid()}:{outer_mode}:{inner_mode}'
                inner_key = key + ':refused'
                keys.extend((key, inner_key))
                seed = vt.phase_enter(args.device, key, mode='eager')
                value = torch.full((1024,), 19, dtype=torch.uint8, device=args.device)
                event.record(stream); event.synchronize()
                seeded = vt.phase_leave(args.device, seed, completed=True, key=key)
                assert seeded['published'] and seeded['observation_id'] == seed
                del value
                stream.synchronize(); vt.trim(args.device)
                previous = vt.phase_stats(args.device, key)
                previous_inner = vt.phase_stats(args.device, inner_key)
                outer = vt.phase_enter(args.device, key, mode=outer_mode)
                active = True
                body_calls = 0
                try:
                    try:
                        vt.phase_enter(args.device, inner_key, mode=inner_mode)
                    except vt.PhaseOverlapError as error:
                        assert type(error) is vt.PhaseOverlapError
                        refusal = {'type': type(error).__name__, 'message': str(error)}
                    else:
                        raise AssertionError('overlapping entry unexpectedly succeeded')
                    assert body_calls == 0
                    assert vt.phase_stats(args.device, key) == previous
                    assert vt.phase_stats(args.device, inner_key) == previous_inner
                    empty = ordinary_refusal(lambda: vt.phase_enter(args.device, '', mode=inner_mode))
                    invalid_mode = ordinary_refusal(
                        lambda: vt.phase_enter(args.device, inner_key, mode='not-a-phase'), ValueError)
                    wrong_token = ordinary_refusal(
                        lambda: vt.phase_leave(args.device, outer + 1, completed=True, key=key))
                    assert vt.phase_stats(args.device, key) == previous
                    # The caller may now choose its existing eager body exactly once.
                    body_calls += 1
                    value = torch.full((2048,), 37, dtype=torch.uint8, device=args.device)
                    event.record(stream); event.synchronize()
                    observed = vt.phase_leave(args.device, outer, completed=True, key=key)
                    active = False
                    assert observed['observation_id'] == outer and observed['mode'] == outer_mode
                    assert not observed['invalidated'] and body_calls == 1
                    assert observed['published'] == (outer_mode == 'eager')
                    after = vt.phase_stats(args.device, key)
                    if outer_mode == 'preparation':
                        assert after == previous
                    else:
                        assert after['successful_passes'] == previous['successful_passes'] + 1
                    assert torch.equal(value.cpu(), torch.full((2048,), 37, dtype=torch.uint8))
                    next_token = vt.phase_enter(args.device, inner_key, mode=inner_mode)
                    assert next_token == outer + 1, 'failed entry consumed a token'
                    closed = vt.phase_leave(args.device, next_token, completed=False, key=inner_key)
                    assert closed['observation_id'] == next_token
                    report['cases'].append({'outer_mode': outer_mode, 'inner_mode': inner_mode,
                        'refusal': refusal, 'empty_key': empty, 'unknown_mode': invalid_mode,
                        'wrong_token': wrong_token, 'original_sample': previous,
                        'original_outer_close': observed, 'subsequent_token': next_token,
                        'authored_body_calls': body_calls})
                    del value
                    stream.synchronize(); vt.trim(args.device)
                finally:
                    if active:
                        vt.phase_leave(args.device, outer, completed=False, key=key)
        report.update(passed=True, gpu_overlap_qualified=True, gpu_executed=True,
                      gpu_uuid=actual_uuid, scope='actual four-pair observation overlap only; no automatic-controller/model qualification')
    finally:
        stream.synchronize()
        for key in keys:
            vt.phase_forget(args.device, key)
print(json.dumps(report), flush=True)
