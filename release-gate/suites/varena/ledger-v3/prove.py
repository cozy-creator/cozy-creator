"""Real old/new processes, OFD waiters and crash/rename faults; supplied bytes are metadata."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--builds', type=Path, required=True)
parser.add_argument('--out', type=Path, required=True)
args = parser.parse_args()
args.out.mkdir(parents=True, exist_ok=False)
builds = json.loads(args.builds.read_text())['builds']
events, actors = [], []


class Actor:
    def __init__(self, version, mode, path, priority=0, wait_ready=True):
        self.process = subprocess.Popen([builds[version]['binary'], mode, str(path), str(priority)],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.pidfd = os.pidfd_open(self.process.pid)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.buffer = b''
        actors.append(self)
        self.ready = self.read() if wait_ready else None

    def read(self):
        while b'\n' not in self.buffer:
            assert self.selector.select(5), 'protocol receipt absent; not a death inference'
            data = os.read(self.process.stdout.fileno(), 65536)
            assert data, {'pid': self.process.pid, 'stderr': self.process.stderr.read().decode()}
            self.buffer += data
        line, self.buffer = self.buffer.split(b'\n', 1)
        value = json.loads(line)
        events.append({'pid': self.process.pid, 'result': value})
        return value

    def send(self, command):
        events.append({'pid': self.process.pid, 'command': command})
        self.process.stdin.write(command.encode() + b'\n'); self.process.stdin.flush()

    def call(self, command):
        self.send(command)
        return self.read()

    def signal(self, sig):
        signal.pidfd_send_signal(self.pidfd, sig)
        events.append({'pid': self.process.pid, 'signal': sig})

    def kill(self):
        if self.process.poll() is None:
            self.signal(signal.SIGKILL)
        self.process.wait(timeout=5)


def waiting_in_old_publication(actor, inode):
    deadline = time.monotonic() + 3
    while True:
        text = Path(f'/proc/{actor.process.pid}/syscall').read_text().strip()
        fields = text.split()
        # Linux x86_64/aarch64 fcntl. This old operation has one blocking OFD lock:
        # publication byte0, after the exact product check() and local mutex.
        if len(fields) >= 3 and fields[0] in ('72', '25') and int(fields[2], 0) == fcntl.F_OFD_SETLKW:
            fd = int(fields[1], 0)
            assert Path(f'/proc/{actor.process.pid}/fd/{fd}').stat().st_ino == inode
            receipt = {'pid': actor.process.pid, 'syscall': text, 'old_inode': inode,
                       'passed_old_check_and_waiting_on_publication': True}
            events.append(receipt)
            return receipt
        assert time.monotonic() < deadline, {'did_not_observe_real_old_waiter': text}
        time.sleep(.001)


report = {'gpu_execution': False, 'cases': {}}
try:
    with tempfile.TemporaryDirectory(prefix='xs138-ledger-v3-', dir='/dev/shm') as root:
        root = Path(root)
        live = root / 'live'; live.mkdir(mode=0o700)
        old = Actor('v2', 'hold', live)
        assert old.ready['ready']
        denied = Actor('v3', 'try', live).ready
        assert 'older ledger participant is still active' in denied['error'], denied
        old.signal(signal.SIGSTOP)
        stopped = Actor('v3', 'try', live).ready
        assert 'older ledger participant is still active' in stopped['error'], stopped
        assert old.process.poll() is None
        old.signal(signal.SIGCONT)
        assert old.call('close')['closed']
        migrated = Actor('v3', 'hold', live)
        assert migrated.ready['ready']
        assert migrated.ready['table']['inode'] != old.ready['table']['inode']
        assert migrated.ready['table']['sample'] is None
        known = migrated.call('sample 64')
        assert known['sample']['reserve'] == 64
        stale = old.call('reap')
        assert 'backing inode changed' in stale['error'], stale
        old_new_open = Actor('v2', 'try', live).ready
        assert 'incompatible ledger' in old_new_open['error']
        assert migrated.call('snapshot')['sample'] == known['sample']
        report['cases']['live_stopped_and_new_old_open'] = {
            'live_refusal': denied, 'stopped_refusal': stopped, 'migrated': migrated.ready,
            'old_mapped_refusal': stale, 'old_new_open_refusal': old_new_open, 'known_v3_sample': known}

        race = root / 'race'; race.mkdir(mode=0o700)
        observer = Actor('v2', 'observer', race)
        path = Path(observer.ready['path'])
        migration = Actor('v3', 'migrate-held', path)
        assert migration.ready['prepared']
        observer.send('reap')
        wait = waiting_in_old_publication(observer, observer.ready['table']['inode'])
        migration.send('commit')
        assert migration.read()['committed']
        refused = observer.read()
        assert 'uncommitted table' in refused['error'], refused
        after = Actor('v3', 'try', race).ready
        assert after['ready']
        report['cases']['old_transaction_already_waiting'] = {'wait': wait, 'refused': refused, 'new': after}

        registration = root / 'registration'; registration.mkdir(mode=0o700)
        observer = Actor('v2', 'observer', registration)
        migration = Actor('v3', 'migrate-held', Path(observer.ready['path']))
        observer.send('register')
        wait = waiting_in_old_publication(observer, observer.ready['table']['inode'])
        migration.send('commit'); assert migration.read()['committed']
        refused = observer.read()
        assert 'uncommitted table' in refused['error']
        registered = Actor('v3', 'hold', registration)
        assert registered.ready['table']['rows'][0]['slot'] == 0
        report['cases']['old_register_already_waiting'] = {'wait': wait, 'refused': refused, 'new': registered.ready}

        opening = root / 'opening'; opening.mkdir(mode=0o700)
        observer = Actor('v2', 'observer', opening)
        migration = Actor('v3', 'migrate-held', Path(observer.ready['path']))
        opener = Actor('v2', 'observer', opening, wait_ready=False)
        wait = waiting_in_old_publication(opener, observer.ready['table']['inode'])
        migration.send('commit'); assert migration.read()['committed']
        refused = opener.read()
        assert 'incompatible ledger' in refused['error']
        report['cases']['old_open_already_waiting'] = {'wait': wait, 'refused': refused}

        red = root / 'rename-only-control'; red.mkdir(mode=0o700)
        observer = Actor('v2', 'observer', red)
        migration = Actor('v3', 'migrate-held', Path(observer.ready['path']))
        observer.send('register')
        wait = waiting_in_old_publication(observer, observer.ready['table']['inode'])
        migration.send('rename-only-control'); assert migration.read()['committed']
        wrongly_admitted = observer.read()
        assert wrongly_admitted == {'registered': True}
        affected = Actor('v3', 'hold', red)
        # v2 opened its row FD by the current pathname while publishing into the detached
        # old map: new table slot0 is locked with no matching row. This is the real red.
        assert affected.ready['table']['rows'][0]['slot'] == 1, affected.ready
        observer.call('close')
        recovered_slot = Actor('v3', 'hold', red)
        assert recovered_slot.ready['table']['rows'][0]['slot'] == 0
        report['cases']['rename_only_negative_control'] = {'wait': wait, 'wrongly_admitted': wrongly_admitted,
            'ghost_lock_forced_slot1': affected.ready, 'slot0_available_after_old_handle_close': recovered_slot.ready,
            'expected_control_failure': True, 'product_uses_tombstone': True}

        crash = root / 'crash'; crash.mkdir(mode=0o700)
        observer = Actor('v2', 'observer', crash)
        migration = Actor('v3', 'migrate-held', Path(observer.ready['path']))
        assert migration.call('tombstone')['tombstoned']
        migration.kill()
        before_recovery = Actor('v2', 'try', crash).ready
        assert 'incompatible ledger' in before_recovery['error']
        recovered = Actor('v3', 'try', crash).ready
        assert recovered['ready'] and recovered['table']['sample'] is None
        report['cases']['crash_after_tombstone'] = {'old_refused': before_recovery, 'recovered': recovered}

        rollback = root / 'rollback'; rollback.mkdir(mode=0o700)
        observer = Actor('v2', 'observer', rollback)
        migration = Actor('v3', 'migrate-held', Path(observer.ready['path']))
        temporary = list(rollback.glob(f'.*.upgrade-{migration.process.pid}-*'))
        assert len(temporary) == 1
        # Actual ENOENT at rename, confined to this fixture's own new temporary inode.
        temporary[0].unlink()
        observer.send('reap')
        wait = waiting_in_old_publication(observer, observer.ready['table']['inode'])
        migration.send('commit')
        failed = migration.read()
        assert 'No such file' in failed['error'], failed
        assert observer.read() == {'reaped': 0}
        restored = observer.call('snapshot')
        assert restored['inode'] == observer.ready['table']['inode']
        report['cases']['rename_failure_restores_old_table'] = {'wait': wait, 'failed': failed, 'restored': restored}

        roles = root / 'roles'; roles.mkdir(mode=0o700)
        active = Actor('v3', 'hold', roles, 0)
        idle = Actor('v3', 'hold', roles, 0)
        requester = Actor('v3', 'hold', roles, 10)
        active.call('policy active no')
        idle.call('policy idle yes')
        requester.call('sample 96')
        selected = requester.call('ask')
        idle_slot = idle.ready['table']['rows'][-1]['slot']
        assert selected['round']['donors'][0] == idle_slot, selected
        assert any(row['activity'] == 'Idle' and row['requestable'] for row in selected['rows'])
        assert requester.call('sample unknown')['sample']['reserve'] == 96
        report['cases']['activity_and_noncrediting_requestability'] = selected
        requester.call('withdraw')
        higher_order = Actor('v3', 'hold', roles, -10)
        higher_order.call('policy active no')
        requester.call('sample unknown')
        explicit_priority = requester.call('ask')
        assert explicit_priority['round']['donors'][0] == higher_order.ready['table']['rows'][-1]['slot']
        before_flag = requester.call('snapshot')['sample']
        only_action = higher_order.call('requestable')
        own = next(row for row in only_action['rows'] if row['pid'] == higher_order.process.pid)
        assert own['requestable'] and own['reclaimable'] == 0
        assert only_action['sample'] == before_flag
        report['cases']['explicit_priority_preserved'] = explicit_priority
        report['cases']['requestable_is_not_capacity'] = only_action

        sequential = root / 'sequential-actions'; sequential.mkdir(mode=0o700)
        first, second, caller = (Actor('v3','hold',sequential,priority) for priority in (-10,0,10))
        first.call('publish 64 0 0 yes idle'); second.call('publish 64 0 0 yes idle')
        caller.call('sample 96'); start = caller.call('ask')
        a, b = start['round']['assignments']
        assert a['action'] and a['answer']=='Waiting' and a['requested']==32
        assert not b['action'] and b['answer']=='Unasked' and b['requested']==0
        pending = first.call('ack action 0 0 pending')
        assert pending['round']['completed']==0 and pending['sample']['used']==512
        assert pending['round']['assignments'][1]['answer']=='Unasked'
        assert 'stale' in second.call('ack action 32 0 complete')['error']
        declined = first.call('ack action 0 0 refused')
        assert declined['round']['assignments'][1]['action'] and declined['round']['assignments'][1]['requested']==32
        duplicate = first.call('ack action 0 0 refused')
        assert duplicate['round']==declined['round']
        second.call('publish 32 0 0 yes idle'); done = second.call('ack action 32 0 complete')
        assert done['round']['finished'] and done['round']['completed']==32 and done['sample']['used']==512
        report['cases']['sequential_owner_actions_no_promised_credit'] = {'start':start,'pending':pending,'declined':declined,'done':done}

        two_stage = root / 'ready-then-owner'; two_stage.mkdir(mode=0o700)
        donor, caller = Actor('v3','hold',two_stage,0), Actor('v3','hold',two_stage,10)
        donor.call('publish 48 16 16 yes idle'); caller.call('sample 96'); start=caller.call('ask')
        assert start['round']['assignments'][0]['requested']==16 and not start['round']['assignments'][0]['action']
        donor.call('publish 48 0 0 yes idle'); action=donor.call('ack ready 16 0 complete')
        assignment=action['round']['assignments'][0]
        assert assignment['action'] and assignment['requested']==32 and assignment['completed']==16 and not action['round']['finished']
        stale=donor.call('ack ready 16 0 complete');assert 'stale' in stale['error']
        donor.call('publish 32 0 0 yes idle');done=donor.call('ack action 32 0 complete')
        assert done['round']['completed']==32 and done['round']['finished']
        report['cases']['ready_stage_then_same_owner_action']={'start':start,'action':action,'old_stage_refused':stale,'done':done}

        all_idle = root/'all-idle';all_idle.mkdir(mode=0o700)
        donor, caller=Actor('v3','hold',all_idle,0),Actor('v3','hold',all_idle,10)
        donor.call('publish 48 16 16 yes idle');caller.call('sample 96');start=caller.call('ask-idle')
        assert start['round']['kind']=='AllIdle' and start['round']['requested']==1024
        donor.call('publish 48 0 0 yes idle');donor.call('ack ready 16 0 complete')
        rejected=donor.call('ack action 1024 0 complete');assert 'inconsistent' in rejected['error']
        donor.call('publish 0 0 0 no idle');done=donor.call('ack action 64 0 refused')
        assert done['round']['completed']==64 and done['round']['finished'] and done['sample']['used']==512
        report['cases']['all_idle_actual_device_bound_and_donor_cap']={'start':start,'over_cap_refused':rejected,'done':done}

        died = root/'action-owner-exit';died.mkdir(mode=0o700)
        first, second, caller=(Actor('v3','hold',died,priority)for priority in (-10,0,10))
        first.call('publish 64 0 0 yes idle');second.call('publish 64 0 0 yes idle');caller.call('sample 96')
        caller.call('ask');first.kill();assert caller.call('reap')['reaped']==1
        advanced=caller.call('snapshot')
        assert advanced['round']['completed']==0 and advanced['sample']['used']==512
        assert advanced['round']['assignments'][0]['answer']=='OwnerGone'
        assert advanced['round']['assignments'][1]['answer']=='Waiting' and advanced['round']['assignments'][1]['action']
        report['cases']['owner_exit_advances_without_credit']=advanced
        report['passed'] = True
        for actor in actors:
            actor.kill()
finally:
    for actor in actors:
        actor.kill()
        actor.selector.close()
        actor.process.stdin.close(); actor.process.stdout.close(); actor.process.stderr.close()
        os.close(actor.pidfd)
    (args.out / 'events.json').write_text(json.dumps(events, indent=2) + '\n')
    (args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
