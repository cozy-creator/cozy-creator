"""Validate actual terminal records, including the GPU function's CUDA device."""
import json
import hashlib
import os
from pathlib import Path
import re
import sys
from urllib.parse import parse_qs, urlsplit

from hardware_types import ArithmeticResult, Candidate, read_installed, read_run, read_sdk


def candidate(value: str, distribution: str) -> Candidate:
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

def main() -> None:
    proof = Path(sys.argv[1])
    requested: dict[str, Candidate] = {'runtime': candidate(sys.argv[2], 'cozy_runtime'),
                                     'tensorfs': candidate(sys.argv[3], 'tensorfs')}
    installed = read_installed(proof / 'candidate-update.ndjson')
    versions = {'runtime': installed['runtime'], 'tensorfs': installed['tensorfs']}
    digests = {'runtime': installed.get('runtime_sha256'), 'tensorfs': installed['tensorfs_sha256']}
    for component, expected in requested.items():
        assert versions[component] == expected['version'], (component, expected, installed)
        if expected['sha256']:
            assert digests[component] == expected['sha256'], (component, expected, installed)
    results: dict[str, ArithmeticResult] = {}
    for name in ('cpu', 'gpu'):
        record = read_run(proof / f'{name}.json')
        assert record['status'] == 'completed', (name, record)
        output = record['result']
        results[name] = output
        assert output['result'] == 368 and output['device'] == ('cuda:0' if name == 'gpu' else 'cpu'), (name, output)
        software = record['measurements']['software']
        assert software['runtime'] == requested['runtime']['version'] and software['tensorfs'] == requested['tensorfs']['version'], software
        if name == 'gpu':
            gpus = record['measurements']['execution']['gpus']
            assert any(gpu['uuid'] == os.environ['CUDA_VISIBLE_DEVICES'] for gpu in gpus), gpus
    sdk = read_sdk(proof / 'operator-sdk/verified.json')
    assert sdk['status'] == 'passed' and sdk['gpu_uuid'] == os.environ['CUDA_VISIBLE_DEVICES'], sdk
    assert sdk['sdk_version'] == requested['tensorfs']['version'] and sdk['runtime_version'] == requested['runtime']['version'], sdk
    assert sdk['wheel_sha256'] == installed['tensorfs_sha256'], sdk
    assert sdk['cpu'] == {'device': 'cpu', 'products': [8., 18.]}, sdk
    assert sdk['gpu'] == {'device': 'cuda:0', 'products': [8., 18.]}, sdk
    for name in ('extension', 'torch_shim'):
        payload = sdk['native_payloads'][name]
        assert re.fullmatch('[a-f0-9]{64}', payload['sha256']) and payload['sha256'] == payload['wheel_sha256'], payload
    (proof / 'verified.json').write_text(json.dumps({'cpu': results['cpu'], 'gpu': results['gpu'], 'requested': requested, 'installed': installed,
        'operator_sdk': sdk, 'qualification': 'CLI/Runtime CUDA and separate operator SDK CUDA; no provider lifecycle qualification'}, indent=2) + '\n')
    print('verified CLI/Runtime CUDA, separate SDK CUDA and exact wheel payloads: ' + json.dumps(installed, sort_keys=True))


if __name__ == '__main__':
    main()
