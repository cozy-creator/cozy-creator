"""Validate actual terminal records, including the GPU function's CUDA device."""
import json
import os
from pathlib import Path
import sys

proof = Path(sys.argv[1])
results = {}
for name in ('cpu', 'gpu'):
    record = json.loads((proof / f'{name}.json').read_text())
    assert record['status'] == 'completed', (name, record.get('reason'))
    output = record['result']
    results[name] = output
    assert output == {'result': 368, 'device': 'cuda:0' if name == 'gpu' else 'cpu'}, (name, output)
    if name == 'gpu':
        gpus = record['measurements']['execution']['gpus']
        assert any(gpu['uuid'] == os.environ['CUDA_VISIBLE_DEVICES'] for gpu in gpus), gpus
(proof / 'verified.json').write_text(json.dumps({'cpu': results['cpu'], 'gpu': results['gpu'],
    'qualification': 'local physical CUDA execution; no provider lifecycle qualification'}, indent=2) + '\n')
