"""The varena suite's verdict (cells.sh), one of analyze.py's CHECKS; analyze.py already holds every cell to completion,
kernels, time and VRAM against the last pass, and differential/ judges its own arms.
  - every H3 cell produced its whole video;
  - no cell ends with varena backing that holds no page (resident - ready): a canceled prefetch's pages go back with it.
    check.py <gate dir> <SKU>     the same verdict alone"""
import json, os, re, subprocess, sys
from typing import TypeAlias

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..'))
import record  # noqa: E402  (release-gate/record.py: varena's own figures, 0.30's and 0.32's alike)
from runs import GateHistory, GateRuns, KnownFailing, Memory, Run  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
Verdict: TypeAlias = tuple[list[str], list[str]]  # (fails, notes)


class VarenaGate(GateHistory, GateRuns):  # what check() reads of analyze.py's gate
    pass


def memories(run: Run) -> list[Memory]:
    """The run's and its calls' memory records (H3 keeps its rows in its calls)."""
    return [memory for m in run.measurements if (memory := m.get('memory'))]


def videos(recs: dict[str, Run]) -> Verdict:
    fails: list[str] = []
    notes: list[str] = []
    for name, run in recs.items():
        if not re.search(r'(^|-)h3(-15)?$', name):
            continue
        vids = run.files('.mp4')
        if not vids:
            fails.append(f'{name}: no video output'); continue
        p = subprocess.run(['ffprobe', '-v', 'error', '-select_streams', 'v:0', '-count_packets', '-show_entries',
                            'stream=height,nb_read_packets', '-of', 'csv=p=0', vids[-1]], capture_output=True, text=True)
        h, fr = (int(x) for x in p.stdout.strip().split(',')[:2])
        want = 300 if name.endswith('-15') else 120
        (fails if h < 700 or fr < want else notes).append(f'{name}: video {h}p, {fr} frames')
    return fails, notes


def unowned(recs: dict[str, Run]) -> Verdict:
    fails: list[str] = []
    for name, run in recs.items():
        left = sum(record.unowned(m) for m in memories(run))
        if left:
            fails.append(f'{name}: {left / 2**20:.0f} MiB of varena backing holds no page at its end (resident - ready)')
    return fails, []


def check(gate: VarenaGate) -> Verdict:
    recs, sku = gate['runs'], gate.get('sku', '')
    fails: list[str] = []
    notes: list[str] = []
    for f, n in (videos(recs), unowned(recs)):
        fails += f; notes += n
    known: list[KnownFailing] = json.load(open(os.path.join(HERE, '..', '..', 'known-failing.json')))
    parked = [c for k in known if k['sku'] in (sku, '*') for c in k.get('cells', [])]
    if parked:
        notes.append(f'varena: parked on {sku}: {", ".join(parked)}')
    return fails, notes


if __name__ == '__main__':
    out, sku = sys.argv[1].rstrip('/'), sys.argv[2]
    runs: dict[str, Run] = {}
    for line in open(os.path.join(out, 'cells.tsv')):
        name = line.split('\t')[0]
        run = Run.read(os.path.join(out, name + '.json'))
        if run and run.status == 'completed':
            runs[name] = run
    fails, notes = check({'out': out, 'sku': sku, 'runs': runs, 'baseline': {}})
    for n in notes: print('note', n)
    for f in fails: print('FAIL', f)
    print('VARENA', 'FAIL' if fails else 'PASS')
    sys.exit(1 if fails else 0)
