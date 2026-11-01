"""Read-only audit of completed ordinary CLI receipts; never imports GPU libraries."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import zipfile

workspace = Path(sys.argv[1]).resolve()
out = Path(__file__).resolve().parent
leg = workspace / 'outputs/varena-v2-p0-20261031/p3-lifecycle-sparse-leg16-plan/attempt-002'
creator = workspace / 'cozy-creator'
MIB = 1 << 20


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load(path):
    return json.loads(path.read_text())


def records(path):
    text = path.read_text()
    decoder = json.JSONDecoder()
    at, result = 0, []
    while at < len(text):
        while at < len(text) and text[at].isspace():
            at += 1
        if at == len(text):
            break
        value, at = decoder.raw_decode(text, at)
        result.append(value)
    return result


def completed(path):
    values = records(path)
    runs = [r['run'] for r in values if r.get('type') == 'run.completed']
    if not runs:
        runs = [r for r in values if r.get('status') == 'completed']
    assert runs
    return runs[-1]


raw = {}
for category in ('ledger-v3', 'machine-mode'):
    for path in sorted((leg / category).iterdir()):
        if path.is_file():
            raw[str(path.relative_to(leg))] = digest(path)

run = load(leg / 'ledger-v3/owner-actions-25737.json')
detail = load(leg / 'ledger-v3/owner-actions-result.json')
assert run['number'] == 25737 and run['status'] == 'completed'
assert run['result']['passed'] and json.loads(run['result']['detail']) == detail
invocation = load(leg / 'ledger-v3/owner-actions-invocation.json')
assert invocation['returncode'] == 0 and '--gpus=1' in invocation['command']
input_path = Path(invocation['command'][invocation['command'].index('--input') + 1])
assert digest(input_path) == invocation['input_sha256']
assert load(input_path) == invocation['input']
for key, value in invocation['input'].items():
    assert detail['provenance'][key] == value, key

fixture = load(workspace / 'outputs/xs138-p3-ledger-v3/fixture-6e85733ef/manifest.json')
project = Path(fixture['project'])
for relative, expected in fixture['files'].items():
    assert digest(project / relative) == expected, relative
    blob = subprocess.check_output(['git', 'show', f"{fixture['fixture_commit']}:release-gate/suites/varena/owner-runtime/{relative}"], cwd=creator)
    assert hashlib.sha256(blob).hexdigest() == expected, relative

native = load(workspace / 'outputs/xs138-p3-ledger-v3/native-065780245/candidate.json')
worker = load(workspace / 'outputs/xs138-p3-machine-mode/candidate-f57bf6eaa/candidate.json')
assert digest(Path(native['wheel_path'])) == native['wheel_sha256']
assert digest(Path(worker['path'])) == worker['sha256']
assert detail['provenance']['extension_sha256'] == native['extension_sha256']
assert detail['provenance']['shim_sha256'] == native['shim_sha256']
for key in ('tensorfs_version', 'extension_sha256', 'shim_sha256'):
    assert detail['child_ready']['provenance'][key] == detail['provenance'][key]

install = completed(leg / 'ledger-v3/install.ndjson')
assert install['number'] == 25735 and install['status'] == 'completed'
assert install['settled_at_ms'] < run['started_at_ms']
assert install['result']['from']['tensorfs'] == '0.17.16+dev.ga96c7c3e'
assert install['result']['to']['tensorfs_sha256'] == native['wheel_sha256']
install_invocation = load(leg / 'ledger-v3/install-invocation.json')
runtime_path = Path(install_invocation['command'][install_invocation['command'].index('--runtime') + 1])
assert digest(runtime_path) == install['result']['to']['runtime_sha256']
with zipfile.ZipFile(runtime_path) as wheel:
    member = next(name for name in wheel.namelist() if name.endswith('cozy_runtime/internal/cooperation.py'))
    assert hashlib.sha256(wheel.read(member)).hexdigest() == detail['provenance']['runtime_owner_sha256']

assigned = detail['assigned']['assigned']
answer = detail['answer']
donor = detail['assigned']['donor_before']
assert donor['activity'] == 'Active' and donor['reclaimable'] == 0 and donor['requestable']
assert donor['physical']['caches']['live'] == 64 * MIB
assert donor['pid'] == detail['provenance']['pid']
assert assigned['kind'] == 'Allocation' and not assigned['finished'] and assigned['completed'] == 0
assert assigned['generation'] == answer['round']['generation'] == 34
assert answer['round']['finished'] and not answer['round']['aborted']
assert len(answer['round']['donations']) == 1
donation = answer['donation']
assert donation == answer['round']['donations'][0]
assert donation['owner'] == donor['id']
assert donation['offered_at_ask'] == 0 and donation['owner_action'] and donation['requestable']
assert donation['answer'] == 'Complete' and donation['pending'] == 0
assert donation['completed'] == donation['requested'] == answer['round']['completed'] == 32 * MIB
assert detail['capped_donor_completed_bytes'] == 32 * MIB
assert detail['callback_counts'] == {'decline': 1, 'release': 1}
assert detail['callbacks'][0]['owner'] == 'decline' and detail['callbacks'][1]['owner'] == 'release'
assert detail['callbacks'][0]['entered_ns'] < detail['callbacks'][1]['returned_ns']
assert all(value['wanted'] == 32 * MIB for value in detail['callbacks'])
unrelated = detail['before_unrelated_release']['mapped_bytes'] - detail['after_unrelated_release']['mapped_bytes']
assert unrelated == 32 * MIB and detail['unrelated_release_did_not_settle_donor']
assert detail['before_unrelated_release']['cache_mapped_bytes'] == detail['after_unrelated_release']['cache_mapped_bytes'] == 64 * MIB
assert detail['retained_cache']['mapped_bytes'] == detail['retained_cache']['live_bytes'] == 32 * MIB
assert detail['live_readback_bytes'] == 40 * MIB and answer['allocation_readback_bytes'] == 32 * MIB
baseline_fields = ('allocated_bytes', 'mapped_bytes', 'pending_bytes', 'pools', 'event_handles', 'cache_mapped_bytes', 'cache_reserved_bytes', 'caches')
for key in baseline_fields:
    assert detail['final'][key] == detail['initial'][key], key
for key in ('context_joins', 'event_waits', 'ooms', 'driver_physical_ooms'):
    assert detail['final'][key] == detail['initial'][key] == 0, key
for key in ('allocated_bytes', 'mapped_bytes', 'pending_bytes', 'pools', 'event_handles'):
    assert answer['requester_final'][key] == 0, key

mode = load(leg / 'machine-mode/report.json')
assert mode['passed'] and not mode['priority_qualified']
assert mode['before']['status']['mode'] == mode['restored']['status']['mode'] == 'dedicated'
source = load(workspace / 'outputs/xs138-p3-machine-mode/fixture-fea8109-sorata/source.json')
stage = workspace / 'outputs/xs138-p3-machine-mode/fixture-fea8109-sorata'
for relative, expected in source['files'].items():
    assert digest(stage / 'source' / relative) == expected
    blob = subprocess.check_output(['git', 'show', f"{source['source']}:{relative}"], cwd=creator)
    assert hashlib.sha256(blob).hexdigest() == expected
for relative, expected in load(stage / 'bundle/manifest.json').items():
    assert digest(stage / 'bundle' / relative) == expected
app = (stage / 'source/release-gate/suites/varena/idle-lifecycle/app.py').read_text()
assert (stage / 'bundle/a/ci_varena_idle/__init__.py').read_text() == app.replace('__APPLICATION__', 'a')
mode_results = []
previous_start = mode['before']['status']['started_at_ms']
for item in mode['results']:
    desired = item['mode']
    actual = item['detail']
    status = item['machine']['status']
    r = completed(leg / f'machine-mode/gpu-{desired}.stdout')
    assert r == item['run'] and r['result']['passed']
    assert json.loads(r['result']['detail']) == actual
    assert status['mode'] == desired and actual['start_shared'] == (desired == 'shared')
    assert status['commit'] == worker['source_commit']
    assert status['runtime'] == detail['provenance']['runtime_version']
    assert status['tensorfs'] == detail['provenance']['tensorfs_version']
    assert actual['extension_sha256'] == native['extension_sha256'] and actual['owner_active']
    assert status['started_at_ms'] > previous_start
    previous_start = status['started_at_ms']
    row = next(row for row in actual['rows'] if row['id'] == actual['owner_row'])
    assert row['pid'] == actual['birth']['pid'] and row['start_ticks'] == actual['birth']['start_ticks']
    assert row['sharing'].lower() == desired and row['priority'] == actual['start_priority'] == 0
    update = completed(leg / f'machine-mode/update-{desired}.stdout')
    assert update['status'] == 'completed' and update['result']['from'] == update['result']['to']
    assert update['settled_at_ms'] < r['started_at_ms']
    mode_results.append({'mode': desired, 'update_run': update['number'], 'gpu_run': r['number'], 'worker_started_at_ms': status['started_at_ms'], 'executor': actual['birth'], 'worker_parent': actual['parent'], 'row': actual['owner_row']})
assert mode_results[0]['worker_parent'] != mode_results[1]['worker_parent']
assert [r['gpu_run'] for r in mode_results] == [25741, 25743]
assert all(event['returncode'] == 0 for event in load(leg / 'machine-mode/progress.json'))

result = {
    'passed': True,
    'scope': 'independent offline audit; no remote/GPU action, build or installed state mutation',
    'native': native['source_commit'], 'runtime': fixture['runtime_commit'], 'worker': worker['source_commit'],
    'owner_fixture': fixture['fixture_commit'], 'mode_fixture': source['source'],
    'wheel_sha256': native['wheel_sha256'], 'runtime_wheel_sha256': install['result']['to']['runtime_sha256'],
    'worker_sha256': worker['sha256'], 'gpu_uuid': detail['provenance']['gpu_uuid'],
    'owner_run': run['number'], 'owner_execution_ms': run['execution_ms'],
    'donor_identity': {key: donor[key] for key in ('id', 'pid', 'start_ticks')},
    'round': assigned['generation'], 'scoped_credit_bytes': donation['completed'],
    'unrelated_released_bytes': unrelated, 'retained_readback_bytes': detail['live_readback_bytes'],
    'requester_readback_bytes': answer['allocation_readback_bytes'],
    'baseline_fields': {key: detail['final'][key] for key in baseline_fields},
    'working_driver_call_delta': detail['final']['driver_memory_calls'] - detail['initial']['driver_memory_calls'],
    'sampled_device_used_drop_bytes': detail['assigned']['driver_before']['driver_used'] - answer['driver_after']['driver_used'],
    'device_sample_is_not_donor_credit': True,
    'mode_results': mode_results, 'restored_mode': mode['restored']['status']['mode'],
    'priority_qualified': False,
    'limits': [
        'Requester asks explicitly; not an automatic physical-OOM recovery proof.',
        'Owner-action cell temporarily opts its donor Shared; ordinary mode propagation is proved by separate25741/25743.',
        'Requester cleanup kills-if-running then waits; graceful child Peer.close is not proved.',
        'Final donor baseline is quiescent task-owned accounting, not zero whole-process use.',
        'Device-used samples are shared-device observations, not attributed donor release.',
        'No foreign-growth attribution/reserve, first-foreign-allocation guarantee, full model, or shared-client db701 qualification.',
        'Mode cell uses restart and fresh executor births; no in-place owner mode-switch or exact old-process exit assertion.',
        'Mode probes verify storage endpoints, not full16MiB content; periodic row physical bytes may precede that allocation.',
        'Protocol3 reached after ordinary update from a96/v2; no before/after table-image migration receipt in this cell.',
    ],
    'raw_receipt_sha256': raw,
}
(out / 'audit.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps({key: result[key] for key in ('passed', 'owner_run', 'scoped_credit_bytes', 'mode_results', 'restored_mode')}))
