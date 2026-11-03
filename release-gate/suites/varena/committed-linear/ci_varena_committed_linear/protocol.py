"""Actual native execution ownership; no mocked CUDA driver or allocator."""
from __future__ import annotations

import torch
from cozy_runtime.author import Context
from tensorfs.varena import _runtime as varena

from . import Probe, Report, actual, app, report, settle


@app.entrypoint(accelerator=True)
def protocol(payload: Probe, ctx: Context) -> Report:
    request, vt, provenance = actual(payload, ctx)
    device = ctx.device.index or 0
    # Executor startup primes the real GEMM verifier on a background thread. Complete
    # that existing initialization before measuring this unrelated closed operation.
    from cozy_runtime.internal import int8_linear

    startup_before = vt.stats(device)
    assert int8_linear.native(torch, device) is not None
    startup_after = vt.stats(device)
    stream = torch.cuda.current_stream(device)
    stream_id = int(stream.cuda_stream)
    on = torch.device('cuda', device)
    settle(vt, device)
    initial = vt.stats(device)
    result = {'provenance': provenance, 'initial': initial, 'cases': [], 'keys': [],
              'startup_readiness': {'before': startup_before, 'after': startup_after,
                                    'boundary': 'actual int8_linear.native before baseline'}}
    whole, half = 8 << 20, 4 << 20
    menu = [('whole', [whole]), ('half', [half])]
    site, configuration = 'fixture.closed-native/1', 'explicit-u8-fill/1'

    def commit():
        owner = vt.commit_execution(device, configuration, site, menu,
                                    preferred='whole', stream=stream_id)
        for key in (owner.key, site):
            request.memory.remember_working_phase(key, (request,))
            result['keys'].append(key)
        return owner

    def refused(action):
        try:
            action()
        except RuntimeError as error:
            return str(error)
        raise AssertionError('the forbidden native transition was accepted')

    def run(result):
        owner = value = stray = graph = graph_value = region_plan = None
        try:
            with torch.inference_mode():
                owner = commit()
                key, selection = owner.key, owner.selection
                assert not vt.phase_stats(device, key)['known']
                owner.enter()
                nested = refused(commit)
                owner.begin()
                value = torch.empty(whole, dtype=torch.uint8, device=on)
                value.fill_(71)
                submitted = owner.end(submitted=True)
                stream.synchronize()
                observed = owner.finish(completed=True)
                assert bool(torch.all(value.cpu() == 71))
                assert observed['published'] and not observed['invalidated'], observed
                assert len(observed['split_observations']) == 1
                assert observed['split_observations'][0]['published']
                reused = refused(owner.enter)
                result['cases'].append({'case': 'completed-current-key', 'key': key,
                                        'selection': selection, 'submission': submitted,
                                        'phase': observed, 'nested_refusal': nested,
                                        'reuse_refusal': reused})
                value = owner = None
                settle(vt, device)
                # Same current declaration, changed actual capacity: the chooser fixes a
                # different choice, so its current key starts unknown rather than reusing
                # the whole variant's completed watermark.
                cap = vt.stats(device)['mapped_bytes'] + half
                vt.set_limit(device, cap)
                try:
                    owner = commit()
                    changed_key, chosen = owner.key, owner.selection
                    unknown = vt.phase_stats(device, changed_key)
                    assert chosen['choice_id'] == 'half' and changed_key != key and not unknown['known']
                    owner.enter()
                    owner.begin()
                    value = torch.empty(half, dtype=torch.uint8, device=on)
                    value.fill_(59)
                    owner.end(submitted=True)
                    stream.synchronize()
                    observed = owner.finish(completed=True)
                    assert observed['published'] and bool(torch.all(value.cpu() == 59))
                    result['cases'].append({'case': 'changed-choice-current-key', 'cap': cap,
                                            'key': changed_key, 'selection': chosen,
                                            'before_sample': unknown, 'phase': observed})
                finally:
                    vt.set_limit(device, None)
                    value = owner = None
                    settle(vt, device)
                sample = vt.phase_stats(device, key)
                owner = commit()
                assert owner.key == key
                owner.enter()
                owner.begin()
                value = torch.empty(whole, dtype=torch.uint8, device=on)
                value.fill_(83)
                stream.synchronize()
                aborted = owner.abort()
                after = vt.phase_stats(device, key)
                result['cases'].append({'case': 'abort-preserves-success', 'before': sample,
                                        'after': after, 'aborted': aborted})
                assert not aborted['published']
                for field in ('known', 'peak_bytes', 'total_peak_bytes', 'backing_peak_bytes',
                              'successful_passes'):
                    assert sample[field] == after[field], field
                assert after['aborted_passes'] == sample['aborted_passes'] + 1
                value = owner = None
                settle(vt, device)
                # A prohibited reselection invalidates, rather than silently resuming the
                # originally committed operation after its control flow has changed.
                owner = commit()
                owner.enter()
                reselect = refused(lambda: vt.split_select(
                    device, site, menu, preferred='whole', stream=stream_id))
                changed = refused(owner.begin)
                owner.abort()
                result['cases'].append({'case': 'reselection-invalidates',
                                        'reselection_refusal': reselect, 'begin_refusal': changed})
                owner = None
                # Region intent is independent metadata: a changed generation refuses a
                # token committed earlier, even when the operation's current key is equal.
                region_plan = varena.Plan(request.memory.native, [('generation', [])])
                region_plan.activate('generation')
                owner = commit()
                selected_generation = owner.selection['plan_generation']
                assert selected_generation > 0
                region_plan.deactivate()
                before_generation = vt.stats(device)
                generation_failure = refused(owner.enter)
                owner.abort()
                after_generation = vt.stats(device)
                result['cases'].append({'case': 'stale-region-generation',
                                        'selected_generation': selected_generation,
                                        'error': generation_failure, 'before': before_generation,
                                        'after': after_generation})
                assert 'generation changed' in generation_failure
                assert before_generation['allocations'] == after_generation['allocations']
                owner = region_plan = None
                # An actual allocation outside the one declared leaf invalidates the phase.
                owner = commit()
                owner.enter()
                stray = torch.empty(256, dtype=torch.uint8, device=on)
                assert stray.numel() == 256
                failure = refused(owner.begin)
                stray = None
                stream.synchronize()
                owner.abort()
                after = vt.phase_stats(device, key)
                result['cases'].append({'case': 'undeclared-allocation', 'error': failure,
                                        'after': after})
                assert after['successful_passes'] == sample['successful_passes']
                owner = None
                settle(vt, device)
                # A real capture is active while commit attempts capture exclusion. Capture
                # performs no allocation and is drained/reset through the ordinary adapter.
                graph_value = torch.zeros(1, dtype=torch.uint8, device=on)
                graph = torch.cuda.CUDAGraph()
                captured = None
                with vt.graph(graph, reserve_bytes=0):
                    captured = refused(commit)
                    graph_value.add_(1)
                vt.replay(graph)
                torch.cuda.synchronize(device)
                assert int(graph_value.cpu()[0]) == 1
                result['cases'].append({'case': 'actual-capture-refusal', 'error': captured})
                # A different resident graph remains executable during this producer's
                # phase. Its real side-stream replay invalidates this measurement only.
                before_replay = vt.phase_stats(device, key)
                owner = commit()
                owner.enter()
                owner.begin()
                value = torch.empty(whole, dtype=torch.uint8, device=on)
                value.fill_(103)
                replay_stream = torch.cuda.Stream(device=device)
                vt.replay(graph, stream=replay_stream)
                owner.end(submitted=True)
                replay_stream.synchronize()
                stream.synchronize()
                observed = owner.finish(completed=True)
                after_replay = vt.phase_stats(device, key)
                graph_value_host = int(graph_value.cpu()[0])
                correct = bool(torch.all(value.cpu() == 103))
                result['cases'].append({'case': 'independent-replay-during-compute',
                                        'phase': observed, 'before': before_replay,
                                        'after': after_replay, 'graph_value': graph_value_host,
                                        'projection_value_correct': correct})
                assert correct and graph_value_host == 2
                assert observed['invalidated'] and not observed['published']
                assert observed['reason'] == 'graph_replay'
                assert after_replay['successful_passes'] == before_replay['successful_passes']
                assert after_replay['peak_bytes'] == before_replay['peak_bytes']
                owner = value = None
                graph.reset()
                graph = graph_value = None
                settle(vt, device)
                # An external owner/callback invalidates even a correctly declared leaf.
                owner = commit()
                owner.enter()
                owner.begin()
                with vt.external_callback(device):
                    value = torch.empty(whole, dtype=torch.uint8, device=on)
                    value.fill_(97)
                owner.end(submitted=True)
                stream.synchronize()
                observed = owner.finish(completed=True)
                after = vt.phase_stats(device, key)
                result['cases'].append({'case': 'external-callback-invalidates',
                                        'phase': observed, 'after': after})
                assert not observed['published'] and observed['invalidated']
                assert after['successful_passes'] == sample['successful_passes']
                value = None
                owner = None
        finally:
            vt.set_limit(device, None)
            if owner is not None:
                owner.abort()
            if graph is not None:
                graph.reset()
            if region_plan is not None:
                region_plan.deactivate()
            owner = value = stray = graph = graph_value = region_plan = None
            settle(vt, device)
            result['final'] = vt.stats(device)
            assert result['final']['allocated_bytes'] == initial['allocated_bytes']
            assert result['final']['mapped_bytes'] == initial['mapped_bytes']
            assert result['final']['pending_bytes'] == result['final']['pools_bytes'] == 0

    return report(result, run)
