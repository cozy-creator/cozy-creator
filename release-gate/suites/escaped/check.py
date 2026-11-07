"""The escaped suite's verdict (cells.sh), one of analyze.py's CHECKS: every completed GPU cell carries varena's escaped
record (`measurements.memory.stats.varena[].stats.escaped`: per loop, what its passes left live as plain working
blocks, by birth site), and no loop's undeclared escaped bytes pass BOUND or grow more than GROWTH a pass. State a
model keeps across steps is held through `author.retained` (spilled to host and refilled), which the record never counts.
    check.py <gate dir> <SKU>     the same verdict alone"""
import os, sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..'))
import record  # noqa: E402
from runs import Run  # noqa: E402

MIB = 1 << 20
BOUND, GROWTH = 256 * MIB, 16 * MIB


def loops(run):
    """Each loop's record in the run's and its calls' memory, and whether any GPU row carried one."""
    rows = [s for m in run.measurements if m.get('memory') for s in record.rows(m['memory'])]
    return [l for s in rows for l in s.get('escaped') or [] if isinstance(l, dict)], any('escaped' in s for s in rows)


def named(sites):
    return ', '.join(f'{s} {b / MIB:.0f} MiB' for s, b in sites) or 'no site named'


def check(gate):
    fails, notes = [], []
    for name, run in gate['runs'].items():
        if not run.gpus:
            continue
        held, recorded = loops(run)
        if not recorded:
            fails.append(f'{name}: no escaped record (its Runtime or TensorFS predates the tally)')
            continue
        for l in held:
            peak, grew = l.get('escaped_peak_bytes') or 0, l.get('growth_bytes') or 0
            line = (f"{name} {l.get('head')}: {peak / MIB:.0f} MiB kept across {l.get('passes')} passes "
                    f"({named(l.get('escaped_sites') or [])}), {grew / MIB:.1f} MiB more a pass, carried in "
                    f"{(l.get('carried_bytes') or 0) / MIB:.0f} MiB")
            (fails if peak > BOUND or grew > GROWTH else notes).append(line)
    return fails, notes


if __name__ == '__main__':
    out, sku = sys.argv[1].rstrip('/'), sys.argv[2]
    runs = {}
    for line in open(os.path.join(out, 'cells.tsv')):
        cell = line.split('\t')[0]
        run = Run.read(os.path.join(out, cell + '.json'))
        if run and run.status == 'completed':
            runs[cell] = run
    fails, notes = check({'out': out, 'sku': sku, 'runs': runs})
    for n in notes: print('note', n)
    for f in fails: print('FAIL', f)
    print('ESCAPED', 'FAIL' if fails else 'PASS')
    sys.exit(1 if fails else 0)
