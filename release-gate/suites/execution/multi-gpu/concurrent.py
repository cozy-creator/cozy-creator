"""PASS when the given run records all completed, each on its own GPU and executor, and all executed at once.

concurrent.py <record.json>...  prints `PASS|FAIL <what happened>`."""
import json
import sys
from typing import TypedDict


class Gpu(TypedDict):
    uuid: str
    pid: int
    start_us: int
    end_us: int


class Execution(TypedDict, total=False):
    gpus: list[Gpu] | None


class Measurements(TypedDict, total=False):
    execution: Execution | None


class Record(TypedDict, total=False):
    """A run's record (`cozy run show --json`), the fields read here."""
    number: int
    status: str
    measurements: Measurements | None


rows: list[tuple[int | None, str, int, int, int]]
bad: list[str]
rows, bad = [], []
for path in sys.argv[1:]:
    rec: Record = json.load(open(path))
    gpus = ((rec.get('measurements') or {}).get('execution') or {}).get('gpus') or []
    if rec.get('status') != 'completed' or len(gpus) != 1:
        bad.append(f"{path}: {rec.get('status')}, {len(gpus)} GPU(s)")
        continue
    g = gpus[0]
    rows.append((rec.get('number'), g['uuid'], g['pid'], g['start_us'], g['end_us']))
uuids, pids = {r[1] for r in rows}, {r[2] for r in rows}
together = rows and max(r[3] for r in rows) < min(r[4] for r in rows)
if len(uuids) < len(rows):
    bad.append(f'{len(rows)} runs on {len(uuids)} GPUs')
if len(pids) < len(rows):
    bad.append(f'{len(rows)} runs in {len(pids)} executors')
if rows and not together:
    bad.append('they did not all execute at once')
span = (max(r[4] for r in rows) - min(r[3] for r in rows)) / 1e6 if rows else 0
runs = ', '.join(f'{n} {(e - s) / 1e6:.0f} s' for n, _, _, s, e in rows)
print(('FAIL ' + '; '.join(bad)) if bad else f'PASS {len(rows)} runs on {len(uuids)} GPUs at once, {span:.0f} s in all ({runs})')
