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
parser.add_argument('--model-arg', action='append', default=[])
args = parser.parse_args()
args.bundle = args.bundle.expanduser().resolve()
args.out = args.out.expanduser().resolve()
if args.model_input is not None:
    args.model_input = args.model_input.expanduser().resolve()
assert args.on not in ('any', 'local') and not args.on.startswith('new')
assert bool(args.model_target) == bool(args.model_input)
common = json.loads((args.bundle / 'common.json').read_text())
expected_old = 'ended' if args.arm == 'candidate' else 'alive'
assert common['gpu_uuid'].startswith(('GPU-', 'MIG-'))
args.out.mkdir(parents=True, exist_ok=False)
records = []


def decode_events(text):
    """CLI failures before a run can be one pretty JSON document, not NDJSON."""
    if not text.strip():
        return [], None
    try:
        return [json.loads(text)], None
    except json.JSONDecodeError:
        values = []
        for number, line in enumerate(text.splitlines(), 1):
            if not line.strip():
                continue
            try:
                values.append(json.loads(line))
            except json.JSONDecodeError as error:
                return values, f'line {number}: {error}'
        return values, None


def call(label, target, payload, fixture=True, arguments=()):
    path = args.out / f'{label}-input.json'
    path.write_text(json.dumps(payload, indent=2) + '\n')
    command = [args.cli, 'run', str(target), *arguments, '--on', args.on, '--gpus=1', '--input', str(path), '--json']
    with (args.out / f'{label}.ndjson').open('w') as stdout, (args.out / f'{label}.stderr').open('w') as stderr:
        result = subprocess.run(command, stdout=stdout, stderr=stderr)
    text = (args.out / f'{label}.ndjson').read_text()
    events, parse_error = decode_events(text)
    record = {'label': label, 'command': command, 'returncode': result.returncode,
              'parse_error': parse_error}
    records.append(record)
    runs = [event['run'] for event in events if isinstance(event, dict)
            and event.get('type') in ('run.completed', 'run.failed', 'run.canceled')]
    if not runs:
        record.update(no_run_receipt=True, cli_events=events)
        (args.out / 'progress.json').write_text(json.dumps(records, indent=2) + '\n')
        raise RuntimeError(f'{label}: CLI exit {result.returncode} before a terminal run receipt; '
                           f'original output saved at {args.out / (label + ".ndjson")}: {text[-2000:]}')
    run = runs[-1]
    (args.out / f'{label}-run.json').write_text(json.dumps(run, indent=2) + '\n')
    with (args.out / f'{label}-show.json').open('w') as stdout, (args.out / f'{label}-show.stderr').open('w') as stderr:
        subprocess.run([args.cli, 'run', 'show', str(run['number']), '--json'], check=False,
                       stdout=stdout, stderr=stderr)
    record.update(number=run['number'], status=run['status'])
    (args.out / 'progress.json').write_text(json.dumps(records, indent=2) + '\n')
    assert result.returncode == 0 and run['status'] == 'completed' and parse_error is None, records[-1]
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
    model = call('model-switch', args.model_target, json.loads(args.model_input.read_text()),
                 fixture=False, arguments=args.model_arg)
    call('after-model', b_target, {**common, 'prior': [{'birth': birth(a_back['birth']), 'expect': expected_old}]})
report = {'passed': True, 'arm': args.arm, 'records': records, 'same_app_reused': True,
          'idle_app_exits_observed': args.arm == 'candidate', 'old_idle_retention_observed': args.arm == 'control',
          'parent_birth_preserved': True, 'model_run': None if model is None else model['number'],
          'scope': 'ordinary sequential calls; parent is an import-only proof only when run launch mode is fork',
          'remaining': 'verify exact cooperative_idle_exit notes precede matching cold Start; warm and active custody cells separate'}
(args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
