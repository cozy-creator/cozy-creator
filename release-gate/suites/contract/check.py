"""The package contract over every cell the gate ran (ENFORCEMENT §3), one of analyze.py's CHECKS for every gate: it reads
the records the other suites made and runs nothing. A cell fails on
- a ContractViolation (the executor's seal: `contract_violation`, the rule in its message);
- an out-of-memory failure in a package's split (its `holds` is short: `working_splits[].refused`, not `pure`);
- a swallowed OOM (`oom_swallowed` warning, `swallowed_ooms`);
- a thread the package started itself (`contract_thread` warning);
- cross-step state left as plain tensors in a run that also declared `author.retained` (a partial port).
A Runtime Retry (`finer_splits`) and a decode through Transformers' loop (a model `lm` does not serve yet) are notes.
    check.py <gate dir>     the same verdict alone"""
import json, os, re, sys
from typing import TypedDict

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..'))
import record  # noqa: E402
from runs import GateDir, Run  # noqa: E402

MIB = 1 << 20
# Escaped bytes past this in a run that declared `retained` are state its port left out (the Runtime's own size floor).
PARTIAL = 64 * MIB
OOM = ('device_out_of_memory', 'memory_exhausted')


class Event(TypedDict, total=False):
    """A line of <cell>.ndjson, the run's event stream."""
    type: str
    level: str
    text: str | None


def warnings(out: str, cell: str) -> list[tuple[str, str]]:
    """The run's warning logs (<cell>.ndjson `run.log`), as (code, text)."""
    path = os.path.join(out, cell + '.ndjson')
    rows: list[tuple[str, str]] = []
    for line in open(path, errors='replace') if os.path.exists(path) else ():
        try:
            event: Event = json.loads(line)
        except ValueError:
            continue
        if event.get('type') == 'run.log' and event.get('level') in ('warning', 'warn'):
            text = event.get('text') or ''
            code = re.search(r'code=(\S+)', text)
            rows.append((code.group(1) if code else '', text))
    return rows


def judge(out: str, cell: str, run: Run) -> tuple[list[str], list[str]]:
    fails: list[str]
    notes: list[str]
    fails, notes = [], []
    code, _, message = run.reason.partition(': ')
    rows = [s for m in run.measurements if m.get('memory') for s in record.rows(m['memory'])]
    if code == 'contract_violation':
        fails.append(f'{cell}: run {run.number} {message[:300]}')
    if run.status == 'failed' and code in OOM:
        for split in (s for r in rows for s in r.get('working_splits') or [] if isinstance(s, dict)):  # type: ignore[redundant-expr]  # unvalidated JSON: other shapes are skipped
            refused = split.get('refused')
            if isinstance(refused, dict):
                who = 'a Runtime estimate (its pure op had no Retry)' if refused.get('pure') else "the package's split: its holds is short"
                (notes if refused.get('pure') else fails).append(
                    f"{cell}: run {run.number} ran out of memory in {split.get('site')} at level {refused.get('level')}, {who}")
    for kind, text in warnings(out, cell):
        if kind in ('oom_swallowed', 'contract_thread'):
            fails.append(f'{cell}: run {run.number} {text[:300]}')
        elif text.startswith("decode ran through Transformers' loop"):
            notes.append(f'{cell}: {text[:200]} (a model lm does not serve yet)')
    swallowed = sum(r.get('swallowed_ooms') or 0 for r in rows)
    if swallowed and not any(k == 'oom_swallowed' for k, _ in warnings(out, cell)):
        fails.append(f'{cell}: run {run.number} returned past {swallowed} refused allocation(s)')
    retried = sum(r.get('finer_splits') or 0 for r in rows)
    if retried:
        notes.append(f'{cell}: {retried} Runtime split(s) ran again finer: an estimate to fix')
    if any(r.get('retained_bytes') for r in rows):
        for loop in (l for r in rows for l in r.get('escaped') or [] if isinstance(l, dict)):  # type: ignore[redundant-expr]  # unvalidated JSON: other shapes are skipped
            if (loop.get('escaped_peak_bytes') or 0) > PARTIAL:
                sites = ', '.join(f'{s} {b / MIB:.0f} MiB' for s, b in loop.get('escaped_sites') or []) or 'no site named'
                fails.append(f"{cell}: {loop.get('head')} declares author.retained and still keeps "
                             f"{loop['escaped_peak_bytes'] / MIB:.0f} MiB across its steps as plain tensors ({sites}): a partial port")
    return fails, notes


def check(gate: GateDir) -> tuple[list[str], list[str]]:
    fails: list[str]
    notes: list[str]
    fails, notes = [], []
    out = gate['out']
    for line in open(os.path.join(out, 'cells.tsv')):
        cell = line.split('\t')[0]
        run = Run.read(os.path.join(out, cell + '.json'))
        if run is not None:
            f, n = judge(out, cell, run)
            fails += f; notes += n
    return fails, notes


if __name__ == '__main__':
    fails, notes = check({'out': sys.argv[1].rstrip('/')})
    for n in notes: print('note', n)
    for f in fails: print('FAIL', f)
    print('CONTRACT', 'FAIL' if fails else 'PASS')
    sys.exit(1 if fails else 0)
