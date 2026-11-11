"""Validate actual terminal records, including the GPU function's CUDA device."""
import json
import hashlib
import os
from pathlib import Path
import re
import sys
from urllib.parse import parse_qs, urlsplit


def candidate(value, distribution):
    source = urlsplit(value)
    name = Path(source.path).name
    digest = None
    if name.endswith('.whl'):
        assert name.startswith(distribution + '-'), value
        version = name[len(distribution) + 1:].split('-')[0]
        if source.scheme:
            digest = parse_qs(source.fragment).get('sha256', [''])[0]
            assert re.fullmatch('[a-f0-9]{64}', digest), 'candidate URL needs its exact sha256'
        else:
            digest = hashlib.sha256(Path(value).read_bytes()).hexdigest()
    else:
        version = value.removeprefix('v')
    return {'version': version, 'sha256': digest}

proof = Path(sys.argv[1])
requested = {'runtime': candidate(sys.argv[2], 'cozy_runtime'), 'tensorfs': candidate(sys.argv[3], 'tensorfs')}
updates = [json.loads(line) for line in (proof / 'candidate-update.ndjson').read_text().splitlines() if line.startswith('{')]
installed = [update['run']['result']['to'] for update in updates if update.get('type') == 'run.completed'][-1]
for component, expected in requested.items():
    assert installed[component] == expected['version'], (component, expected, installed)
    if expected['sha256']:
        assert installed[component + '_sha256'] == expected['sha256'], (component, expected, installed)
results = {}
for name in ('cpu', 'gpu'):
    record = json.loads((proof / f'{name}.json').read_text())
    assert record['status'] == 'completed', (name, record.get('reason'))
    output = record['result']
    results[name] = output
    assert output['result'] == 368 and output['device'] == ('cuda:0' if name == 'gpu' else 'cpu'), (name, output)
    assert output['tensorfs_products'] == [8., 18.], (name, output)
    assert re.fullmatch('[a-f0-9]{64}', output['tensorfs_native_sha256']), output
    assert output['tensorfs_revision'].startswith('sha256:'), output
    for component, expected in requested.items():
        assert record['measurements']['software'][component] == expected['version'], (name, component, record['measurements']['software'])
    if name == 'gpu':
        assert re.fullmatch('[a-f0-9]{64}', output['torch_shim_sha256']), output
        gpus = record['measurements']['execution']['gpus']
        assert any(gpu['uuid'] == os.environ['CUDA_VISIBLE_DEVICES'] for gpu in gpus), gpus
assert results['cpu']['tensorfs_native_sha256'] == results['gpu']['tensorfs_native_sha256'], results
(proof / 'verified.json').write_text(json.dumps({'cpu': results['cpu'], 'gpu': results['gpu'], 'requested': requested, 'installed': installed,
    'qualification': 'local physical CUDA execution; no provider lifecycle qualification'}, indent=2) + '\n')
print('verified physical CUDA and exact candidate cohort: ' + json.dumps(installed, sort_keys=True))
