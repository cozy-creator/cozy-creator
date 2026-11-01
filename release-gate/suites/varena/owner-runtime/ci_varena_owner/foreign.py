"""Real GPU categorical switch cell; no claim of native foreign-process sampling here."""
import gc
import json
import os
import time


def mode_switch(payload, ctx):
    from tensorfs import varena
    from . import MIB, Report, Requester, current, eventually

    torch, vt, _memory, owner, provenance = current(payload, ctx)
    native = owner._peer
    assert native.pressure_capability() == 1
    torch.cuda.synchronize(); vt.trim(0)
    initial = vt.stats(0)
    child = None
    idle = None
    try:
        config = {key: getattr(payload, key) for key in (
            'tensorfs_version', 'extension_sha256', 'shim_sha256', 'gpu_uuid')}
        config.update(directory=str(owner.config.directory), donor_pid=os.getpid(), wanted=64 * MIB)
        child = Requester(config, 'mode_requester.py')
        ready = child.read()
        assert ready['ready']
        eventually(lambda: (native.configure(shared=True, priority=-10), True)[1])
        idle = torch.full((64 * MIB,), 37, dtype=torch.uint8, device='cuda')
        assert int(idle[0]) == int(idle[-1]) == 37
        torch.cuda.synchronize(); del idle; gc.collect()

        def published():
            table = native.snapshot()
            row = next(row for row in table['rows'] if row['id'] == table['owner'])
            return row if row['reclaimable'] >= config['wanted'] else None

        donor = eventually(published)
        # A real native exclusion guard prevents release before the categorical switch.
        # This is not presented as an actual CUDA graph-recording proof.
        with varena.capture(0):
            before_calls = native.cuda_calls()
            assigned = child.ask()['assigned']
            assert any(d['owner'] == donor['id'] for d in assigned['donations'])
            assert assigned['completed'] == 0
            eventually(lambda: (native.configure(shared=False, priority=-10), True)[1])
            acknowledged = native.snapshot()
            after_calls = native.cuda_calls()
            assert after_calls == before_calls, (before_calls, after_calls)
            assert next(row for row in acknowledged['rows'] if row['id'] == donor['id'])['sharing'] == 'Dedicated'
            at_ack = vt.stats(0)
        child.process.stdin.write(b'observe\n'); child.process.stdin.flush()
        refused = child.read()['round']
        donation = next(d for d in refused['donations'] if d['owner'] == donor['id'])
        assert donation['answer'] == 'Refused' and donation['completed'] == donation['pending'] == 0
        # Observe more than one native cadence after acknowledgement: an old assignment
        # must not start another release once configure(Dedicated) has returned.
        time.sleep(1.1)
        after = vt.stats(0)
        assert after['mapped_bytes'] == at_ack['mapped_bytes'], (at_ack, after)
        detail = {'provenance': provenance, 'fixture_mode_opt_in': True,
                  'native_exclusion_guard': True, 'actual_cuda_graph': False,
                  'child': ready, 'donor_before': donor, 'assigned': assigned,
                  'dedicated_ack': acknowledged, 'refused': refused,
                  'calls_during_exclusion': after_calls - before_calls,
                  'at_ack': at_ack, 'after_ack': after,
                  'no_release_after_dedicated_ack': True}
    finally:
        idle = None
        if child is not None:
            child.close()
        eventually(lambda: (native.configure(shared=owner.config.shared, priority=owner.config.priority), True)[1])
        gc.collect(); torch.cuda.synchronize(); vt.trim(0)
    final = vt.stats(0)
    for key in ('allocated_bytes', 'mapped_bytes', 'pending_bytes', 'pools', 'event_handles'):
        assert final[key] == initial[key], (key, initial, final)
    detail.update(initial=initial, final=final)
    return Report(True, json.dumps(detail, sort_keys=True))


def steady_foreign(payload, ctx):
    from . import MIB, Report, Requester, current, eventually

    torch, vt, _memory, owner, provenance = current(payload, ctx)
    native = owner._peer
    assert native.pressure_capability() == 1
    torch.cuda.synchronize(); vt.trim(0)
    initial = vt.stats(0)
    child = None
    phase = None
    idle = live = None
    try:
        eventually(lambda: (native.configure(shared=True, priority=-10), True)[1])
        baseline = eventually(lambda: native.facts()['pressure'] if native.facts()['pressure']['samples'] else None)
        assert baseline['attribution'] == 'nvml_host_pid_sampled', {
            'unqualified': 'steady-foreign requires real host-PID NVML attribution', 'facts': baseline}
        assert torch.cuda.mem_get_info()[0] >= 2 * 1024 * MIB, 'cell requires measured free capacity'
        child = Requester({'bytes': 512 * MIB}, 'foreign_process.py')
        ready = child.read()
        assert ready['ready'] and not ready['cuda_initialized']
        idle = torch.full((64 * MIB,), 37, dtype=torch.uint8, device='cuda')
        torch.cuda.synchronize(); del idle; gc.collect()
        before = vt.stats(0)
        assert before['mapped_bytes'] >= initial['mapped_bytes'] + 64 * MIB
        baseline = native.facts()['pressure']
        allocated = child.ask()
        records = []

        def released():
            facts, state = native.facts()['pressure'], vt.stats(0)
            records.append({'observed_ns': time.monotonic_ns(), 'pressure': facts, 'working': state})
            return facts if (facts['process_signals_served'] > baseline['process_signals_served']
                             and state['mapped_bytes'] == initial['mapped_bytes']) else None

        first = eventually(released)
        release_observed_ns = time.monotonic_ns()
        assert child.process.poll() is None
        assert first['foreign'] >= 512 * MIB
        assert first['reserve'] is None or first['free'] >= 2 * first['reserve'], {
            'unqualified': 'steady cell requires ample measured free memory after foreign growth', 'facts': first}
        # The threshold event has completed. Holding the same foreign allocation must not
        # turn newly idle demand pages into an endless sequence of pressure donations.
        idle = torch.full((64 * MIB,), 39, dtype=torch.uint8, device='cuda')
        torch.cuda.synchronize(); del idle; gc.collect()
        retained = vt.stats(0)
        samples = first['samples']
        eventually(lambda: True if native.facts()['pressure']['samples'] >= samples + 3 else None)
        steady = native.facts()['pressure']
        after = vt.stats(0)
        assert after['mapped_bytes'] == retained['mapped_bytes'], (retained, after, steady)
        assert steady['process_signals'] == first['process_signals']
        assert steady['suppressed'], 'a sampler tick raised retention without a phase boundary'
        phase = vt.phase_enter(0, 'xs138-steady-foreign-regrowth')
        raised = native.facts()['pressure']
        assert not raised['suppressed'], raised
        live = torch.full((64 * MIB,), 43, dtype=torch.uint8, device='cuda')
        assert int(live[0]) == int(live[-1]) == 43
        torch.cuda.synchronize(); del live; gc.collect()
        phase_result = vt.phase_leave(0, phase, completed=True)
        phase = None
        vt.phase_forget(0, 'xs138-steady-foreign-regrowth')
        assert child.process.poll() is None
        child.process.stdin.write(b'quit\n'); child.process.stdin.flush()
        foreign_end = child.read()
        assert foreign_end['reserved'] == 0
        detail = {'provenance': provenance, 'fixture_mode_opt_in': True,
                  'foreign_ready': ready, 'foreign_allocation': allocated, 'foreign_end': foreign_end,
                  'initial': initial, 'before': before, 'observations': records,
                  'first_episode': first, 'retained': retained, 'steady': steady, 'after': after,
                  'raised_at_phase_entry': raised, 'phase_result': phase_result,
                  'release_observed_ns': release_observed_ns,
                  'allocation_start_to_release_observed_ns': release_observed_ns - allocated['allocation_start_ns'],
                  'latency_scope': 'upper-bound observation from foreign allocation start; includes allocation itself',
                  'steady_foreign_did_not_redrain': True}
    finally:
        idle = live = None
        if phase is not None:
            vt.phase_leave(0, phase, completed=False)
            vt.phase_forget(0, 'xs138-steady-foreign-regrowth')
        if child is not None:
            child.close()
        eventually(lambda: (native.configure(shared=owner.config.shared, priority=owner.config.priority), True)[1])
        gc.collect(); torch.cuda.synchronize(); vt.trim(0)
    final = vt.stats(0)
    for key in ('allocated_bytes', 'mapped_bytes', 'pending_bytes', 'pools', 'event_handles'):
        assert final[key] == initial[key], (key, initial, final)
    detail['final'] = final
    return Report(True, json.dumps(detail, sort_keys=True))
