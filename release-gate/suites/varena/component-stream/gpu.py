"""Real component-stream contract gates; requires Agent1's shared wiring and exact provider.

No model download, fake stream/event, allocator policy or application retry. Run in
an owned isolated child with its existing external protocol deadline.
"""
from __future__ import annotations

import argparse
import cProfile
import gc
import hashlib
import importlib.util
import json
import os
import pstats
from pathlib import Path

import torch
from tensorfs.varena import api
from tensorfs.varena import torch as vt
from tensorfs.varena._offload import OffloadError
from tensorfs.varena import _offload_stream as scopes

parser = argparse.ArgumentParser()
parser.add_argument('--device', type=int, default=0)
parser.add_argument('--gpu-uuid', required=True)
parser.add_argument('--provider', type=Path, required=True)
parser.add_argument('--provider-python', type=Path, required=True)
parser.add_argument('--provider-sha256', required=True)
parser.add_argument('--provider-python-sha256', required=True)
args = parser.parse_args()
sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
assert sha(args.provider) == args.provider_sha256
assert sha(args.provider_python) == args.provider_python_sha256
assert not torch.cuda.is_initialized()
vt.install(args.device)
torch.cuda.set_device(args.device)
actual_uuid = str(torch.cuda.get_device_properties(args.device).uuid)
normalize = lambda value: value.removeprefix('GPU-').replace('-', '').lower()
assert normalize(actual_uuid) == normalize(args.gpu_uuid)
spec = importlib.util.spec_from_file_location('real_component_gate_provider', args.provider_python)
provider_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provider_module)
provider = provider_module.Provider(args.provider)
model = torch.nn.Sequential(*[torch.nn.Linear(4, 4) for _ in range(8)]).eval()
with torch.no_grad():
    for index, parameter in enumerate(model.parameters()):
        parameter.copy_(torch.arange(parameter.numel()).reshape(parameter.shape) / 64 + index / 64)
host_input = torch.arange(16, dtype=torch.float32).reshape(4, 4) / 16
with torch.inference_mode():
    expected = model(host_input).clone()
handle = vt.offload(model, device=args.device)
owner = handle._owner
gate = provider.create(owner.gpu, 1 << 20)
x = host_input.to(owner.device)
caller = torch.cuda.current_stream(owner.device)
explicit = torch.cuda.Stream(device=owner.device)
changed = torch.cuda.Stream(device=owner.device)
later = torch.cuda.Stream(device=owner.device)
report = {'passed': False, 'gpu_uuid': actual_uuid, 'pid': os.getpid(), 'checks': [],
          'scope': 'component stream mechanism and existing lifetime contract; no model benchmark'}
try:
    with torch.inference_mode():
        # Fixed warmup completes real weights/library setup before introducing a held gate.
        with scopes.component_scope(owner, (x,)) as warm:
            warm_result = warm.output(handle(x))
        warm.completion.wait()
        torch.testing.assert_close(warm_result.cpu(), expected, rtol=1e-5, atol=1e-6)
        del warm_result
        profile = cProfile.Profile()
        profile.enable()
        with scopes.component_scope(owner, (x,)) as outer:
            selected = outer.stream
            with scopes.component_scope(owner, (x,)) as inner:
                assert inner.reused and inner.stream == selected
                result = inner.output(handle(x))
                cold = inner.checkpoint()
                cold.wait()
                assert cold.checked and cold.can_train
                assert scopes.reuse(owner, selected), "cold checkpoint closed the outer scope"
            assert inner.completion is None, "nested steady exit created a completion event"
            result = outer.output(result)
        profile.disable()
        outer.completion.wait()
        assert outer.completion.can_train and cold.can_train
        stats = pstats.Stats(profile).stats
        def calls(function):
            code = function.__code__
            return stats.get((code.co_filename, code.co_firstlineno, code.co_name), (0, 0))[1]
        reuse_calls = calls(scopes.reuse)
        contexts = calls(torch.cuda.StreamContext.__enter__)
        assert reuse_calls >= 8, 'Agent1 _Device.execution reuse wiring is not present'
        assert contexts == 1, (contexts, 'nested/per-op stream contexts were not amortized')
        torch.testing.assert_close(result.cpu(), expected, rtol=1e-5, atol=1e-6)
        report['checks'].append({'nested_contexts': contexts, 'actual_reuse_calls': reuse_calls})

        with torch.cuda.stream(explicit), scopes.component_scope(owner, (x,)) as scope:
            assert scope.stream == explicit
            explicit.wait_stream(caller)  # Standard explicit caller input-ordering obligation.
            explicit_result = scope.output(handle(x))
            explicit_checkpoint = scope.checkpoint()
        assert scope.completion is None, "same-stream steady exit created a completion event"
        explicit_checkpoint.wait()
        torch.testing.assert_close(explicit_result.cpu(), expected, rtol=1e-5, atol=1e-6)
        report['checks'].append({'explicit_caller_honored': True})

        with scopes.component_scope(owner, (x,)) as switched:
            changed.wait_stream(switched.stream)
            with torch.cuda.stream(changed):
                assert not scopes.reuse(owner, changed)
                switched_result = handle(x)
                assert torch.cuda.current_stream(owner.device) == changed
            switched_result = switched.output(switched_result)
        assert not switched.completion.can_train and switched.completion.stream_changed
        switched.completion.wait()
        torch.testing.assert_close(switched_result.cpu(), expected, rtol=1e-5, atol=1e-6)
        report['checks'].append({'actual_change_not_redirected': True, 'changed_cannot_train': True})

        # Real mapped HOST gate: record_stream must never make this a completed sample.
        gate.extra_gate_set(0)
        gate.extra_gate_wait(owner.stream.cuda_stream)
        with scopes.component_scope(owner, (x,)) as held:
            assert held.stream == owner.stream
            held_result = held.output(handle(x))
        assert gate.stream_pending(owner.stream.cuda_stream)
        assert not held.completion.query()
        gate.extra_gate_set(1)
        held.completion.wait()
        assert held.completion.query()
        torch.testing.assert_close(held_result.cpu(), expected, rtol=1e-5, atol=1e-6)
        report['checks'].append({'pending_event_not_completed': True, 'checked_completion': True})

        # An escaped ordinary output view survives actual later-stream consumption.
        view = held_result.view(-1)
        source_pointer = view.data_ptr()
        output = torch.empty_like(view)
        gate.extra_gate_set(0)
        gate.extra_gate_wait(later.cuda_stream)
        assert held.completion.handoff(later, view) is view
        with torch.cuda.stream(later):
            output.copy_(view)
        del view, held_result
        gc.collect()
        vt.trim(args.device)
        replacements = [torch.full_like(output, -13) for _ in range(64)]
        assert all(value.data_ptr() != source_pointer for value in replacements), "pending alias backing was reused by another stream"
        caller.synchronize()
        assert gate.stream_pending(later.cuda_stream)
        gate.extra_gate_set(1)
        later.synchronize()
        torch.testing.assert_close(output.cpu(), expected.reshape(-1), rtol=1e-5, atol=1e-6)
        report['checks'].append({'escaped_alias_later_consumer': True})

        prior = torch.cuda.current_stream(owner.device)
        try:
            with scopes.component_scope(owner, (x,)) as failed:
                handle(x)
                raise ValueError('actual component failure')
        except ValueError:
            pass
        assert torch.cuda.current_stream(owner.device) == prior and not scopes.reuse(owner, prior)
        assert not failed.completion.can_train
        try:
            failed.completion.query()
        except OffloadError:
            pass
        else:
            raise AssertionError('failed body claimed a successful sample')
        prior.synchronize()
        copied = model[0].weight.clone()  # Existing direct managed read outside a component.
        prior.synchronize()
        assert copied.shape == (4, 4)
        report['checks'].append({'exception_restores_context': True, 'direct_read_safe_path': True})
        report['working_after'] = vt.stats(args.device)
        report['passed'] = True
except BaseException as error:
    report["error"] = repr(error)
    raise
finally:
    # Open only this fixture's gates, complete its streams, and preserve failures.
    try:
        gate.gate_set(1)
        gate.extra_gate_set(1)
        for stream in (owner.stream, explicit, changed, later, caller):
            stream.synchronize()
        gate.close()
        handle.close()
    except BaseException as error:
        report["passed"] = False
        report["cleanup_error"] = repr(error)
        raise
    finally:
        print(json.dumps(report), flush=True)
