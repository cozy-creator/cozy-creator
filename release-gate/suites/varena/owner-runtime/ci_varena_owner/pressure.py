"""Real physical pressure through the actual Runtime Memory owner, never manual asks."""
import gc
import os
import signal
import time
from pathlib import Path

from . import MIB, Requester, current, eventually


def exercise(payload, ctx):
    from tensorfs import varena

    cases = {'working', 'weight', 'gil', 'stopped', 'writer', 'cap', 'va', 'limited', 'capture', 'replay'}
    assert payload.case in cases
    torch, vt, memory, owner, provenance = current(payload, ctx)
    owner_row = eventually(owner._peer.snapshot)['owner']
    assert memory.room_limit is None
    assert 'driver_physical_ooms' in vt.stats(0) and 'recoveries' in owner.facts()
    target_bytes = 64 * MIB
    child = None
    holder = None
    gate = stopped = limited = False
    ballast = live = idle = storage = old_view = result = pinned = target_use = None
    arena = varena.Arena(0)
    idle_weight = pinned_weight = target_weight = None
    phase = None
    phase_report = None
    setup = {}
    native_before = native_after = donor_after = None
    operation_error = None

    def fill(region, value):
        use = region.prepare()
        use.tensor.fill_(value)
        use.commit()
        use.consume_on(torch.cuda.current_stream().cuda_stream)

    def tell(command):
        child.process.stdin.write(command.encode() + b'\n')
        child.process.stdin.flush()
        return child.read()

    try:
        warm = torch.full((2 * MIB,), 7, dtype=torch.uint8, device='cuda')
        warm.untyped_storage().resize_(4 * MIB)
        assert int(warm[0]) == 7
        del warm
        torch.cuda.synchronize(); vt.trim(0)
        baseline = vt.stats(0)
        config = {key: getattr(payload, key) for key in ('tensorfs_version', 'extension_sha256', 'shim_sha256', 'gpu_uuid', 'gil_gate_sha256')}
        config.update(directory=str(owner.config.directory), idle_bytes=target_bytes)
        child = Requester(config, 'donor.py')
        ready = child.read()
        assert ready['ready'] and ready['before']['reclaimable'] >= target_bytes
        assert ready['pid'] != os.getpid()

        live = torch.full((8 * MIB,), 23, dtype=torch.uint8, device='cuda')
        pinned_weight = arena.reserve(8 * MIB)
        fill(pinned_weight, 17)
        gc.collect(); torch.cuda.synchronize()
        eventually(lambda: True if arena.stats()['pinned_bytes'] == 0 else None)
        pinned = pinned_weight.pin()
        assert pinned is not None
        idle_weight = arena.reserve(8 * MIB)
        fill(idle_weight, 41)
        target_weight = arena.reserve(target_bytes)
        idle = torch.full((8 * MIB,), 19, dtype=torch.uint8, device='cuda')
        assert int(idle[0]) == int(idle[-1]) == 19
        if payload.case == 'gil':
            storage = torch.UntypedStorage(2 * MIB, device='cuda')
            old_view = torch.empty(0, dtype=torch.uint8, device='cuda').set_(storage, 0, (2 * MIB,), (1,))
            old_view.fill_(7)
        torch.cuda.synchronize(); idle = None; gc.collect()
        eventually(lambda: True if arena.stats()['pinned_bytes'] == 8 * MIB else None)
        setup.update(working=vt.stats(0), weights=arena.stats(), owner=owner.facts(), donor=ready)

        pressure = payload.case in {'working', 'weight', 'gil', 'stopped', 'writer', 'limited'}
        if pressure:
            free, total = torch.cuda.mem_get_info(0)
            quantum = max(2 * MIB, arena.granularity)
            ballast_bytes = (free - 16 * MIB) // quantum * quantum
            assert ballast_bytes > target_bytes
            # Test setup uses an actual driver observation, not a managed cap or policy reserve.
            ballast = torch.empty((ballast_bytes,), dtype=torch.uint8, device='cuda')
            assert ballast.numel() == ballast_bytes
            setup.update(driver_free_before_ballast=free, driver_total=total, ballast_bytes=ballast_bytes,
                         driver_after_ballast=torch.cuda.mem_get_info(0))
            assert vt.stats(0)['driver_physical_ooms'] == setup['working']['driver_physical_ooms'], 'setup itself hit pressure'
            assert owner.facts()['recoveries'] == setup['owner']['recoveries'], 'setup asked peers'
        elif payload.case == 'cap':
            vt.set_limit(0, vt.stats(0)['mapped_bytes'] + arena.stats()['resident_bytes'])
            limited = True

        if payload.case == 'gil':
            held = tell('gil')
            assert held == {'gil_gate': True, 'gil_held': 1}, held
            gate = True
        elif payload.case == 'stopped':
            os.kill(child.process.pid, signal.SIGSTOP)
            pid, status = os.waitpid(child.process.pid, os.WUNTRACED)
            assert pid == child.process.pid and os.WIFSTOPPED(status)
            stopped = True
        elif payload.case == 'writer':
            table = eventually(owner._peer.snapshot)
            paths = [p for p in Path(owner.config.directory).iterdir() if p.is_file()
                     and p.stat().st_dev == table['backing_device'] and p.stat().st_ino == table['backing_inode']]
            assert len(paths) == 1, paths
            holder = Requester({'path': str(paths[0]), 'device': table['backing_device'],
                                'inode': table['backing_inode']}, 'publication_holder.py')
            held = holder.read()
            assert held['publication_held'] and held['pid'] == holder.process.pid
            os.kill(holder.process.pid, signal.SIGSTOP)
            pid, status = os.waitpid(holder.process.pid, os.WUNTRACED)
            assert pid == holder.process.pid and os.WIFSTOPPED(status)
            setup['publication_holder'] = held

        native_before = {'working': vt.stats(0), 'weights': arena.stats(), 'owner': owner.facts()}
        phase_key = f'xs138-physical:{payload.case}:{setup.get("ballast_bytes", 0)}'
        if payload.case in {'working', 'weight', 'gil'}:
            phase = vt.phase_enter(0, phase_key)
        began = time.monotonic_ns()
        try:
            if payload.case in {'weight', 'limited'}:
                target_use = target_weight.prepare(reclaim=payload.case != 'limited')
                target_use.tensor.fill_(31)
                target_use.commit()
                assert int(target_use.tensor[0]) == int(target_use.tensor[-1]) == 31
            elif payload.case == 'gil':
                storage.resize_(target_bytes)  # Real Torch path retains the caller's GIL.
                assert storage.nbytes() == target_bytes and int(old_view[0]) == int(old_view[-1]) == 7
            elif payload.case == 'va':
                _, total = torch.cuda.mem_get_info(0)
                pointer = torch.cuda.caching_allocator_alloc(total * 16)
                torch.cuda.caching_allocator_delete(pointer)
                raise AssertionError('allocation larger than the native VA reservation succeeded')
            elif payload.case in {'capture', 'replay'}:
                graph = torch.cuda.CUDAGraph() if payload.case == 'capture' else None
                stream = torch.cuda.Stream() if graph is not None else None
                scope = vt.graph(graph, stream=stream, reserve_bytes=2 * MIB) if graph is not None else varena.replay(0)
                with scope:
                    calls = owner._peer.cuda_calls()
                    try:
                        result = torch.empty((target_bytes,), dtype=torch.uint8, device='cuda')
                    except RuntimeError as error:
                        operation_error = f'{type(error).__name__}: {error}'
                    assert operation_error is not None, 'guarded allocation unexpectedly succeeded'
                    assert owner._peer.cuda_calls() == calls, 'guarded allocation made a native driver call'
                if graph is not None:
                    graph.reset()
                    del graph, stream
            else:
                result = torch.full((target_bytes,), 29, dtype=torch.uint8, device='cuda')
                assert int(result[0]) == int(result[-1]) == 29
        except (torch.OutOfMemoryError, MemoryError) as error:
            operation_error = f'{type(error).__name__}: {error}'
        elapsed_ns = time.monotonic_ns() - began
        if phase is not None:
            torch.cuda.synchronize()
            phase_report = vt.phase_leave(0, phase, completed=operation_error is None, key=phase_key)
            phase = None
        native_after = {'working': vt.stats(0), 'weights': arena.stats(), 'owner': owner.facts()}
        if gate:
            child.process.stdin.write(b'x'); child.process.stdin.flush()
            gate = False
            assert child.read()['gil_released']
        if stopped:
            assert child.process.poll() is None
            table = eventually(owner._peer.snapshot)
            assert any(row['id'] == ready['row'] and row['pid'] == child.process.pid for row in table['rows'])
            donation = next(row for row in table['round']['donations'] if row['owner'] == ready['row'])
            assert donation['completed'] == 0 and donation['answer'] == 'Waiting', donation
            setup['stopped_donor_at_budget'] = {'pid': child.process.pid, 'row': ready['row'],
                                               'alive': True, 'donation': donation}
            os.kill(child.process.pid, signal.SIGCONT); stopped = False
        if holder is not None:
            assert holder.process.poll() is None
            setup['publication_holder_alive_at_budget'] = True
            os.kill(holder.process.pid, signal.SIGCONT)
            holder.process.stdin.write(b'stop\n'); holder.process.stdin.flush()
            assert holder.read()['publication_released']
            holder.process.wait(timeout=10)
        if payload.case in {'working', 'weight', 'gil'}:
            def donation_ready():
                value = tell('facts')
                donation = value['facts']['last_donation']
                return value if donation is not None and donation['completed'] >= target_bytes else None
            donor_after = eventually(donation_ready)
        else:
            donor_after = tell('facts')
        assert donor_after['live_preserved']
        assert int(live[0]) == int(live[-1]) == 23
        assert int(pinned.tensor[0]) == int(pinned.tensor[-1]) == 17
        attempts = native_after['owner']['recoveries'] - native_before['owner']['recoveries']
        physical_working = native_after['working']['driver_physical_ooms'] - native_before['working']['driver_physical_ooms']
        physical_weights = native_after['weights']['driver_physical_ooms'] - native_before['weights']['driver_physical_ooms']
        if payload.case in {'working', 'weight', 'gil'}:
            assert operation_error is None, operation_error
            assert attempts >= 1 and physical_working + physical_weights >= 3
            assert native_after['owner']['last_recovery']['outcome'] == 'released'
            assert donor_after['facts']['last_donation']['completed'] >= target_bytes
            assert not idle_weight.resident, 'local idle weights were not reclaimed before asking'
            assert native_after['working']['ooms'] == native_before['working']['ooms'], 'a recovered allocation counted as terminal OOM'
            assert phase_report['published'] and not phase_report['invalidated'], phase_report
        elif payload.case in {'stopped', 'writer'}:
            assert operation_error is not None and attempts >= 1 and physical_working >= 3
            assert native_after['owner']['last_recovery']['outcome'] == 'peer_wait_budget_exhausted'
            assert native_after['owner']['active'] and native_after['owner']['failed'] is None
        else:
            assert operation_error is not None and attempts == 0
            if payload.case != 'limited':
                assert physical_working == physical_weights == 0
    finally:
        try:
            if gate and child is not None and child.process.poll() is None:
                child.process.stdin.write(b'x'); child.process.stdin.flush()
                assert child.read()['gil_released']
                gate = False
            if stopped and child is not None and child.process.poll() is None:
                os.kill(child.process.pid, signal.SIGCONT)
                stopped = False
            if limited:
                vt.set_limit(0, None)
            if phase is not None:
                vt.phase_leave(0, phase, completed=False)
            if target_use is not None:
                target_use.consume_on(torch.cuda.current_stream().cuda_stream)
            if pinned is not None:
                pinned.consume_on(torch.cuda.current_stream().cuda_stream)
            target_use = pinned = old_view = storage = result = live = idle = ballast = None
            gc.collect(); torch.cuda.synchronize(); vt.trim(0)
            eventually(lambda: True if arena.stats()['pinned_bytes'] == 0 else None)
            arena.reclaim(1 << 30)
        finally:
            if holder is not None:
                holder.close()  # Even a stopped holder releases its OFD lock on owned kill.
            if child is not None:
                try:
                    if not gate and not stopped and child.process.poll() is None:
                        child.process.stdin.write(b'stop\n'); child.process.stdin.flush()
                        assert child.read()['stopped']
                        child.process.wait(timeout=10)
                        assert child.process.returncode == 0
                finally:
                    child.close()
    def settled():
        vt.trim(0)
        value = vt.stats(0)
        return value if value['pending_bytes'] == 0 and value['mapped_bytes'] == baseline['mapped_bytes'] else None
    final = eventually(settled)
    assert final['allocated_bytes'] == baseline['allocated_bytes'] and final['pending_bytes'] == 0
    assert final['pools'] == baseline['pools'] and final['event_handles'] == 0
    return {'passed': True, 'case': payload.case, 'provenance': provenance, 'owner_row': owner_row, 'setup': setup,
            'before': native_before, 'after': native_after, 'donor_after': donor_after,
            'phase': phase_report, 'operation_error': operation_error, 'allocation_elapsed_ns': elapsed_ns,
            'baseline': baseline, 'final': final,
            'scope': 'real native physical recovery or explicit non-physical negative; no manual requester ask or donor service; no full P3/game claim'}
