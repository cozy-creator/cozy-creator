"""Real-process device-pressure round proof; supplied byte metadata is not GPU release."""
import argparse
import json
import os
import selectors
import subprocess
import tempfile
import time
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('peer', type=Path)
parser.add_argument('out', type=Path)
args = parser.parse_args()
args.out.mkdir(parents=True, exist_ok=False)
events, actors = [], []

class Actor:
    def __init__(self, directory, priority):
        self.process = subprocess.Popen([str(args.peer.resolve()), 'nonblocking', directory, str(priority)],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.select = selectors.DefaultSelector()
        self.select.register(self.process.stdout, selectors.EVENT_READ)
        self.buffer = b''
        actors.append(self)
        self.ready = self.read()
        assert self.ready['ready']

    def send(self, command):
        events.append({'pid': self.process.pid, 'command': command})
        self.process.stdin.write(command.encode() + b'\n'); self.process.stdin.flush()

    def read(self):
        while b'\n' not in self.buffer:
            assert self.select.select(5), 'ledger peer did not respond'
            value = os.read(self.process.stdout.fileno(), 65536)
            assert value, self.process.stderr.read().decode()
            self.buffer += value
        line, self.buffer = self.buffer.split(b'\n', 1)
        value = json.loads(line); events.append(value)
        return value

    def call(self, command):
        deadline = time.monotonic() + 5
        while True:
            self.send(command); value = self.read()
            if value.get('error') != 'ledger publication is busy': return value
            assert time.monotonic() < deadline
            time.sleep(.001)

try:
    with tempfile.TemporaryDirectory(prefix='xs138-pressure-', dir='/dev/shm') as directory:
        a, b, dedicated = Actor(directory, -20), Actor(directory, -10), Actor(directory, -30)
        assert a.call('publish 0 64 0 64 0')['ok']
        assert b.call('publish 0 64 0 64 0')['ok']
        assert dedicated.call('publish 0 128 0 128 0')['ok']
        assert dedicated.call('configure dedicated -30')['ok']
        sample = a.call('sample 512 256 1')['sample']
        command = f'ask-device {sample} 96'
        a.send(command); b.send(command)  # Both real processes compete for the same observation.
        first, second = a.read(), b.read()
        if first.get('error') == 'ledger publication is busy': first = a.call(command)
        if second.get('error') == 'ledger publication is busy': second = b.call(command)
        assert first['round'] == second['round']
        snapshot = a.call('snapshot'); round_ = snapshot['round']
        assert round_['completed'] == 0 and sum(d['requested'] for d in round_['donors']) == 96
        ids = [d['slot'] for d in round_['donors']]
        assert ids == [a.ready['slot'], b.ready['slot']], round_
        assert snapshot['sample']['reserve'] is None and not snapshot['sample']['attributable']
        rejected = dedicated.call(command)
        assert rejected.get('error') == 'invalid ledger operation: device pressure requires a Shared participant', rejected
        assert a.call(command)['round'] == first['round']
        assert a.call('publish 0 0 0 0 0')['ok']
        assert a.call(f'ack {first["round"]} 64 0 complete')['ok']
        assert b.call('publish 0 32 0 32 0')['ok']
        assert b.call(f'ack {first["round"]} 32 0 complete')['ok']
        completed = a.call('snapshot')
        assert completed['round']['completed'] == 96 and completed['round']['finished']
        assert completed['sample']['used'] == 256, 'acknowledgements changed driver capacity'
        # Ordinary allocation recovery retains its original exclusion of the requester.
        assert a.call('publish 0 64 0 64 0')['ok']
        fresh = a.call('sample 512 224 2')['sample']
        ordinary = a.call(f'ask {fresh} 64')
        after = a.call('snapshot')
        assert ordinary['round'] != first['round']
        assert [d['slot'] for d in after['round']['donors']] == [b.ready['slot']]
        # Declared measured metadata only: this proves native aggregation/validation,
        # not NVML attribution or the empirical value of H.
        assert 'sample' in a.call('sample-reserve 512 224 3 64 1')
        assert 'sample' in b.call('sample 512 224 4')
        preserved = b.call('snapshot')
        assert preserved['sample']['reserve'] == 64 and preserved['sample']['attributable']
        assert 'sample' in b.call('sample-reserve 512 224 5 32 1')
        assert a.call('snapshot')['sample']['reserve'] == 64
        assert 'sample' in b.call('sample-reserve 512 224 6 96 1')
        assert a.call('snapshot')['sample']['reserve'] == 96
        rejected_h = a.call('sample-reserve 512 224 7 128 0')
        assert rejected_h.get('error') == 'invalid ledger operation: reserve requires an attributable observation'
        assert a.call('snapshot')['sample']['reserve'] == 96
        report = {'passed': True, 'device_round': snapshot, 'completed_metadata': completed,
                  'ordinary_allocation_round': after, 'dedicated_refusal': rejected,
                  'same_round_for_two_processes': True, 'self_included_without_double_deficit': True,
                  'unknown_headroom_preserved': True, 'driver_capacity_not_credited': True,
                  'unknown_sampler_preserves_qualified_device_max': preserved,
                  'lower_reserve_does_not_lower_device_max': True,
                  'unattributable_reserve_rejected': rejected_h,
                  'gpu_execution': False, 'scope': 'real processes, OFD row/publication locks and native ledger; supplied byte metadata only'}
finally:
    for actor in actors:
        if actor.process.poll() is None: actor.process.kill()
        actor.process.wait(timeout=5); actor.select.close()
    (args.out / 'events.json').write_text(json.dumps(events, indent=2) + '\n')
(args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
