"""Known-goods that predate to_step1_ms (load + step 1) take it from their own gate's run records.
    backfill_to_step1.py <baseline file> <gate dir>...
Each gate dir is a gate's output (its `label`, each cell's <name>.json); an entry whose label and run number match a record
there gets to_step1_ms from it, and nothing else changes. Entries with no record left keep none: the gate notes the cell
starts that number."""
import json, os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runs import Run  # noqa: E402

base_path, dirs = sys.argv[1], sys.argv[2:]
base = json.load(open(base_path))
filled = 0
for d in dirs:
    label = open(os.path.join(d, 'label')).read().strip()
    for name, cell in base['cells'].items():
        for g in cell.get('good', []):
            if g.get('to_step1_ms') is not None or g.get('label') != label:
                continue
            run = Run.read(os.path.join(d, f'{name}.json'))
            if run and run.number == g.get('run') and run.to_step1_ms is not None:
                g['to_step1_ms'] = run.to_step1_ms
                filled += 1
                print(f'{name} ({label}, run {run.number}): to_step1_ms {run.to_step1_ms}')
json.dump(base, open(base_path, 'w'), indent=1)
print(f'{filled} entries filled')
