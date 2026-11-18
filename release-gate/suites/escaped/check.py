"""The escaped suite's verdict (cells.sh), one of analyze.py's CHECKS: every completed capped GPU cell carries varena's
escaped record (`measurements.memory.stats.varena[].stats.escaped`: per loop, what its passes left live as plain working
blocks, by birth site), and no loop's undeclared escaped bytes pass BOUND or grow more than GROWTH a pass. State a
model keeps across steps is held through `author.retained` (spilled to host and refilled), which the record never counts.
A cell that failed or was refused memory capped has an uncapped twin (<cell>-uncapped, cells.sh): the twin completes only
if the capped cell did, with the same kernels, and the capped one's driver peak stays within CAP and a CUDA context:
success, peak and kernels, never the bytes (a cap may tile). A capped run varena moved live tensors to host pages for
(cr-305 rung 5) has a twin too, and its output against the twin's is stated: byte-identical, or each frame's difference.
    check.py <gate dir> <SKU>     the same verdict alone"""
import os, sys
from collections.abc import Callable
from typing import TypeGuard

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..'))
import record  # noqa: E402
from runs import EscapedLoop, GateRuns, Run  # noqa: E402

MIB, GIB = 1 << 20, 1 << 30
BOUND, GROWTH = 256 * MIB, 16 * MIB
# cells.sh's cap, and what a CUDA context and the libraries outside varena add to the driver's count beside it.
CAP, CONTEXT = 6 << 30, 1 << 30
TWIN = '-uncapped'


def loops(run: Run) -> tuple[list[EscapedLoop], bool]:
    """Each loop's record in the run's and its calls' memory, and whether any GPU row carried one."""
    rows = [s for m in run.measurements if m.get('memory') for s in record.rows(m['memory'])]
    return [l for s in rows for l in s.get('escaped') or [] if isinstance(l, dict)], any('escaped' in s for s in rows)


def moved(run: Run) -> int:
    """The live working bytes varena moved to host pages for the run (cr-305 rung 5): its records' allocator deltas."""
    rows = [s for m in run.measurements if m.get('memory') for s in record.rows(m['memory'])]
    return max((int((s.get('allocator') or {}).get('demoted_total') or 0) for s in rows), default=0)


def same(capped: Run, uncapped: Run) -> str:
    """A moved run's output against its twin's: the bytes, else the first video's per-frame RGB difference."""
    def digests(run: Run) -> list[str]:
        return sorted(str(o.get('sha256') or '') for o in run.record.get('output') or [] if isinstance(o, dict))
    if digests(capped) == digests(uncapped):
        return 'byte-identical'
    videos = [(a, b) for a, b in zip(capped.outputs, uncapped.outputs) if a.endswith('.mp4') and b.endswith('.mp4')]
    if not videos:
        return 'its outputs differ, with no video of both to compare'
    import frames  # numpy and ffmpeg, only when a twin differs
    p = frames.pair(*videos[0])
    return f"differs: each frame's mean RGB difference {p['mean']} of 255, the worst {p['worst']} at frame {p['worst_at']}"


def named(sites: list[tuple[str, int]]) -> str:
    return ', '.join(f'{s} {b / MIB:.0f} MiB' for s, b in sites) or 'no site named'


def twins(out: str) -> list[tuple[str, Run | None, Run | None]]:
    """Each capped cell's run beside its uncapped twin's: (cell, capped, uncapped), either None when it has no record."""
    names = [line.split('\t')[0] for line in open(os.path.join(out, 'cells.tsv'))]
    read: Callable[[str], Run | None] = lambda n: Run.read(os.path.join(out, n + '.json'))
    return [(n[:-len(TWIN)], read(n[:-len(TWIN)]), read(n)) for n in names if n.endswith(TWIN)]


def stopped(out: str) -> set[str]:
    """The cells gate.sh stopped after their steps on purpose (stopped.tsv, xs-208): canceled there, judged as ran."""
    p = os.path.join(out, 'stopped.tsv')
    return {line.split('\t')[0] for line in open(p) if '\t' in line} if os.path.exists(p) else set()


def ran(run: Run | None, name: str, meant: set[str]) -> TypeGuard[Run]:
    """It completed, or gate.sh stopped it after its steps."""
    return run is not None and (run.status == 'completed' or (run.status == 'canceled' and name in meant))


def compared(out: str) -> tuple[list[str], list[str]]:
    fails: list[str]
    notes: list[str]
    fails, notes = [], []
    meant = stopped(out)
    for cell, capped, uncapped in twins(out):
        if not ran(uncapped, cell + TWIN, meant):
            continue
        if not ran(capped, cell, meant):
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
        if gone := moved(capped):
            notes.append(f'{cell}: {gone / GIB:.1f} GiB of live tensors on host pages (rung 5); against uncapped run '
                         f'{uncapped.number}: {same(capped, uncapped)}')
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
