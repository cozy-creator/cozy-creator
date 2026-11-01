"""Real three-process ledger regression. Byte values are metadata, never GPU evidence."""
import argparse
import json
import os
import selectors
import signal
import subprocess
import tempfile
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('peer', type=Path)
parser.add_argument('out', type=Path)
parser.add_argument('--fixed', action='store_true')
args = parser.parse_args()
args.out.mkdir(parents=True, exist_ok=False)
events = []
actors = []

class Actor:
    def __init__(self, directory, priority):
        self.process = subprocess.Popen([str(args.peer.resolve()), 'nonblocking', directory, str(priority)],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.select = selectors.DefaultSelector()
        self.select.register(self.process.stdout, selectors.EVENT_READ)
        self.buffer = b''
        actors.append(self)
        self.ready = self.read()

    def read(self):
        while b'\n' not in self.buffer:
            assert self.select.select(5), 'ledger peer did not answer'
            value = os.read(self.process.stdout.fileno(), 65536)
            assert value, self.process.stderr.read().decode()
            self.buffer += value
        line, self.buffer = self.buffer.split(b'\n', 1)
        value = json.loads(line); events.append(value)
        assert 'error' not in value, value
        return value

    def call(self, command):
        events.append({'pid': self.process.pid, 'command': command})
        self.process.stdin.write(command.encode() + b'\n'); self.process.stdin.flush()
        return self.read()

try:
    with tempfile.TemporaryDirectory(prefix='xs138-round-', dir='/dev/shm') as directory:
        requester = Actor(directory, 0)
        a = Actor(directory, -10)
        b = Actor(directory, -20)
        a.call('publish 0 64 0 64 0')
        b.call('publish 0 64 0 64 0')
        b.call('configure dedicated -20')
        sample = requester.call('sample 256 128 1')['sample']
        first = requester.call(f'ask {sample} 64')['round']
        os.kill(a.process.pid, signal.SIGSTOP)
        pid, status = os.waitpid(a.process.pid, os.WUNTRACED)
        assert pid == a.process.pid and os.WIFSTOPPED(status)
        b.call('configure shared -20')
        before = requester.call('snapshot')
        assert before['round']['donors'][0]['slot'] == a.ready['slot']
        assert before['round']['donors'][0]['answer'] == 'Waiting'
        if args.fixed:
            foreign_sample = b.call('sample 256 128 3')['sample']
            assert b.call(f'ask {foreign_sample} 16')['round'] == first
            b.call('clear-want')
            foreign_cleared = requester.call('snapshot')
            assert not foreign_cleared['round']['aborted'] and foreign_cleared['round']['id'] == first
            assert next(row for row in foreign_cleared['rows'] if row['slot'] == b.ready['slot'])['want'] == 0
            assert next(row for row in foreign_cleared['rows'] if row['slot'] == requester.ready['slot'])['want'] == 64
            assert b.call(f'withdraw {first}')['changed'] is False
            assert requester.call(f'withdraw {first+1000}')['changed'] is False
            assert requester.call(f'withdraw {first}')['changed'] is True
            assert requester.call(f'withdraw {first}')['changed'] is False
            retired = requester.call('snapshot')
            assert retired['round']['aborted'] and retired['round']['completed'] == 0
            assert retired['sample']['used'] == before['sample']['used'] == 128
        sample = requester.call('sample 256 128 2')['sample']
        second = requester.call(f'ask {sample} 64')['round']
        after = requester.call('snapshot')
        if args.fixed:
            assert second != first and after['round']['donors'][0]['slot'] == b.ready['slot'], after
        else:
            assert second == first and after['round']['donors'][0]['slot'] == a.ready['slot'], after
        assert a.process.poll() is None
        report = {'passed': True, 'fixed': args.fixed, 'obsolete_round_reproduced': not args.fixed,
                  'first_round': first, 'second_round': second, 'a_stopped_alive': a.process.pid,
                  'a': a.ready, 'b': b.ready, 'before': before, 'after': after,
                  'gpu_or_allocator_execution': False,
                  'scope': 'real OS participants/row locks and ledger transitions; supplied byte metadata only'}
finally:
    for actor in actors:
        if actor.process.poll() is None: actor.process.kill()
        actor.process.wait(timeout=5)
        actor.select.close()
    (args.out / 'events.json').write_text(json.dumps(events, indent=2) + '\n')
(args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
