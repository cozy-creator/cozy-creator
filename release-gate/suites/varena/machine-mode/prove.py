"""Ordinary machine update and GPU Start proof. Root operates it on an owned machine."""
import argparse
import json
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--cli', default='cozy')
parser.add_argument('--on', required=True)
parser.add_argument('--bundle', type=Path, required=True)
parser.add_argument('--out', type=Path, required=True)
args = parser.parse_args()
assert args.on not in ('local', 'any') and not args.on.startswith('new')
args.out.mkdir(parents=True, exist_ok=False)
common = json.loads((args.bundle / 'common.json').read_text())
assert common['gpu_uuid'].startswith(('GPU-', 'MIG-'))
events = []


def command(label, arguments):
    command = [args.cli, *arguments, '--json']
    with (args.out / f'{label}.stdout').open('w') as stdout, (args.out / f'{label}.stderr').open('w') as stderr:
        result = subprocess.run(command, stdout=stdout, stderr=stderr)
    events.append({'label': label, 'command': command, 'returncode': result.returncode})
    (args.out / 'progress.json').write_text(json.dumps(events, indent=2) + '\n')
    assert result.returncode == 0, events[-1]
    return (args.out / f'{label}.stdout').read_text()


def show(label):
    machine = json.loads(command(label, ['machine', 'show', args.on]))
    assert not machine.get('unreachable'), machine
    status = machine['status']
    assert status['mode'] in ('shared', 'dedicated'), status
    assert status['runtime'] == common['runtime_version'] and status['tensorfs'] == common['tensorfs_version'], status
    return machine


before = show('before')
original = before['status']['mode']
results = []
changed = False
try:
    for mode in ('shared', 'dedicated'):
        changed = True  # A failed update can have persisted settings; cleanup must still observe.
        command(f'update-{mode}', ['machine', 'update', args.on, '--mode', mode])
        machine = show(f'show-{mode}')
        assert machine['status']['mode'] == mode
        payload = {**common, 'expected_shared': mode == 'shared'}
        path = args.out / f'{mode}-input.json'
        path.write_text(json.dumps(payload, indent=2) + '\n')
        stream = command(f'gpu-{mode}', ['run', str(args.bundle / 'a/probe'), '--on', args.on,
                                         '--gpus=1', '--input', str(path)])
        rows = [json.loads(line) for line in stream.splitlines() if line.strip()]
        run = next(row['run'] for row in reversed(rows) if row.get('type') == 'run.completed')
        assert run['result']['passed'], run
        detail = json.loads(run['result']['detail'])
        assert detail['start_shared'] == (mode == 'shared')
        results.append({'mode': mode, 'machine': machine, 'run': run, 'detail': detail})
finally:
    if changed:
        current = show('before-restore')
        if current['status']['mode'] != original:
            command('restore', ['machine', 'update', args.on, '--mode', original])
        restored = show('restored')
        assert restored['status']['mode'] == original
report = {'passed': True, 'before': before, 'results': results, 'restored': restored,
          'scope': 'ordinary update/drain/restart, live status and real Runtime Start/native row; no manual owner configure',
          'priority_qualified': False}
(args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({'passed': True, 'runs': [row['run']['number'] for row in results], 'restored_mode': original}))
