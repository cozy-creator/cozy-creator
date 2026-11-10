"""The escaped suite's verdict (cells.sh), one of analyze.py's CHECKS: every completed capped GPU cell carries varena's
escaped record (`measurements.memory.stats.varena[].stats.escaped`: per loop, what its passes left live as plain working
blocks, by birth site), and no loop's undeclared escaped bytes pass BOUND or grow more than GROWTH a pass. State a
model keeps across steps is held through `author.retained` (spilled to host and refilled), which the record never counts.
A cell that failed or was refused memory capped has an uncapped twin (<cell>-uncapped, cells.sh): the twin completes only
if the capped cell did, with the same kernels, and the capped one's driver peak stays within CAP and a CUDA context:
success, peak and kernels, never the bytes (a cap may tile).
    check.py <gate dir> <SKU>     the same verdict alone"""
import os, sys
from collections.abc import Callable

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..'))
import record  # noqa: E402
from runs import EscapedLoop, GateRuns, Run  # noqa: E402

MIB = 1 << 20
BOUND, GROWTH = 256 * MIB, 16 * MIB
# cells.sh's cap, and what a CUDA context and the libraries outside varena add to the driver's count beside it.
CAP, CONTEXT = 6 << 30, 1 << 30
TWIN = '-uncapped'


def loops(run: Run) -> tuple[list[EscapedLoop], bool]:
    """Each loop's record in the run's and its calls' memory, and whether any GPU row carried one."""
    rows = [s for m in run.measurements if m.get('memory') for s in record.rows(m['memory'])]
    return [l for s in rows for l in s.get('escaped') or [] if isinstance(l, dict)], any('escaped' in s for s in rows)  # type: ignore[redundant-expr]  # unvalidated JSON: other shapes are skipped


def named(sites: list[tuple[str, int]]) -> str:
    return ', '.join(f'{s} {b / MIB:.0f} MiB' for s, b in sites) or 'no site named'


def twins(out: str) -> list[tuple[str, Run | None, Run | None]]:
    """Each capped cell's run beside its uncapped twin's: (cell, capped, uncapped), either None when it has no record."""
    names = [line.split('\t')[0] for line in open(os.path.join(out, 'cells.tsv'))]
    read: Callable[[str], Run | None] = lambda n: Run.read(os.path.join(out, n + '.json'))
    return [(n[:-len(TWIN)], read(n[:-len(TWIN)]), read(n)) for n in names if n.endswith(TWIN)]


def compared(out: str) -> tuple[list[str], list[str]]:
    fails: list[str]
    notes: list[str]
    fails, notes = [], []
    for cell, capped, uncapped in twins(out):
        if uncapped is None or uncapped.status != 'completed':
            continue
        if capped is None or capped.status != 'completed':
            why = capped.reason[:200] if capped else 'no record'
            fails.append(f'{cell}: fails at the {CAP >> 30} GiB cap ({why}) and completes uncapped (run {uncapped.number}): '
                         'it sizes its work from the card, not from varena\'s room')
            continue
        if (capped.gemm, capped.attention) != (uncapped.gemm, uncapped.attention):
            fails.append(f'{cell}: kernels {capped.gemm} / {capped.attention} capped, {uncapped.gemm} / {uncapped.attention} uncapped')
        peak = capped.vram['added'] if capped.vram else None
        line = f'{cell}: capped run {capped.number} added {peak / 1024:.1f} GiB' if peak is not None else f'{cell}: capped run {capped.number}: no driver reading'
        if peak is not None and uncapped.vram:
            line += f', uncapped run {uncapped.number} {uncapped.vram["added"] / 1024:.1f} GiB'
        (fails if peak is not None and peak * MIB > CAP + CONTEXT else notes).append(line)
    return fails, notes


def check(gate: GateRuns) -> tuple[list[str], list[str]]:
    fails, notes = compared(gate['out'])
    for name, run in gate['runs'].items():
        if not run.gpus or name.endswith(TWIN):
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
    runs: dict[str, Run] = {}
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
