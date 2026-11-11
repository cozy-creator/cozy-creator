"""Validate actual terminal records, including the GPU function's CUDA device."""
import json
from pathlib import Path
import sys

proof = Path(sys.argv[1])
results = {}
for name in ('cpu', 'gpu'):
    record = json.loads((proof / f'{name}.json').read_text())
    assert record['status'] == 'completed', (name, record.get('reason'))
    output = record['output']
    results[name] = output
    # Outputs use the CLI's text item representation; retain exact records beside this verdict.
    encoded = json.dumps(output)
    assert '368' in encoded, (name, output)
    assert ('cuda:0' if name == 'gpu' else 'cpu') in encoded, (name, output)
(proof / 'verified.json').write_text(json.dumps({'cpu': results['cpu'], 'gpu': results['gpu'],
    'qualification': 'local physical CUDA execution; no provider lifecycle qualification'}, indent=2) + '\n')
