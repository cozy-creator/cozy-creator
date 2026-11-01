"""Run only through ordinary Cozy on an already owned machine; no install or rental route."""
import argparse
import json
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--bundle', type=Path, required=True)
parser.add_argument('--out', type=Path, required=True)
parser.add_argument('--on', required=True)
parser.add_argument('--cli', default='cozy')
parser.add_argument('--arm', choices=('control', 'candidate'), required=True)
parser.add_argument('--model-target')
parser.add_argument('--model-input', type=Path)
args = parser.parse_args()
assert args.on not in ('any', 'local') and not args.on.startswith('new')
assert bool(args.model_target) == bool(args.model_input)
common = json.loads((args.bundle / 'common.json').read_text())
expected_old = 'ended' if args.arm == 'candidate' else 'alive'
assert common['gpu_uuid'].startswith(('GPU-', 'MIG-'))
args.out.mkdir(parents=True, exist_ok=False)
records = []


def call(label, target, payload, fixture=True):
    path = args.out / f'{label}-input.json'
    path.write_text(json.dumps(payload, indent=2) + '\n')
    command = [args.cli, 'run', str(target), '--on', args.on, '--gpus=1', '--input', str(path), '--json']
    with (args.out / f'{label}.ndjson').open('w') as stdout, (args.out / f'{label}.stderr').open('w') as stderr:
        result = subprocess.run(command, stdout=stdout, stderr=stderr)
    events = [json.loads(line) for line in (args.out / f'{label}.ndjson').read_text().splitlines() if line.strip()]
    runs = [event['run'] for event in events if event.get('type') in ('run.completed', 'run.failed', 'run.canceled')]
    assert runs, events[-3:]
    run = runs[-1]
    (args.out / f'{label}-run.json').write_text(json.dumps(run, indent=2) + '\n')
    with (args.out / f'{label}-show.json').open('w') as stdout, (args.out / f'{label}-show.stderr').open('w') as stderr:
        subprocess.run([args.cli, 'run', 'show', str(run['number']), '--json'], check=False,
                       stdout=stdout, stderr=stderr)
    records.append({'label': label, 'command': command, 'returncode': result.returncode,
                    'number': run['number'], 'status': run['status']})
    (args.out / 'progress.json').write_text(json.dumps(records, indent=2) + '\n')
    assert result.returncode == 0 and run['status'] == 'completed', records[-1]
    if fixture:
        assert run['result']['passed'], run['result']
        return json.loads(run['result']['detail'])
    return run


def birth(row):
    return {key: row[key] for key in ('pid', 'start_ticks', 'boot_id')}


a_target, b_target = (args.bundle / name / 'probe' for name in ('a', 'b'))
a = call('a-first', a_target, common)
a_reuse = call('a-reuse', a_target, {**common, 'expected_pid': a['birth']['pid']})
assert a_reuse['calls'] == a['calls'] + 1 and a_reuse['storage_ptr'] == a['storage_ptr']
b = call('b-switch', b_target, {**common, 'prior': [
    {'birth': birth(a['birth']), 'expect': expected_old}, {'birth': birth(a['parent']), 'expect': 'alive'}]})
assert b['birth']['pid'] != a['birth']['pid']
b_reuse = call('b-reuse', b_target, {**common, 'expected_pid': b['birth']['pid']})
assert b_reuse['calls'] == b['calls'] + 1 and b_reuse['storage_ptr'] == b['storage_ptr']
a_back = call('a-switch-back', a_target, {**common, 'expected_parent': birth(a['parent']), 'prior': [
    {'birth': birth(b['birth']), 'expect': expected_old}, {'birth': birth(a['birth']), 'expect': expected_old}]})
assert a_back['calls'] == (1 if args.arm == 'candidate' else a_reuse['calls'] + 1)
model = None
if args.model_target:
    model = call('model-switch', args.model_target, json.loads(args.model_input.read_text()), fixture=False)
    call('after-model', b_target, {**common, 'prior': [{'birth': birth(a_back['birth']), 'expect': expected_old}]})
report = {'passed': True, 'arm': args.arm, 'records': records, 'same_app_reused': True,
          'idle_app_exits_observed': args.arm == 'candidate', 'old_idle_retention_observed': args.arm == 'control',
          'parent_birth_preserved': True, 'model_run': None if model is None else model['number'],
          'scope': 'ordinary sequential calls; parent is an import-only proof only when run launch mode is fork',
          'remaining': 'verify exact cooperative_idle_exit notes precede matching cold Start; warm and active custody cells separate'}
(args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
