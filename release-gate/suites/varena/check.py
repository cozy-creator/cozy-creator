"""The varena suite's verdict (cells.sh), one of analyze.py's CHECKS; analyze.py already holds every cell to completion,
kernels, time and VRAM against the last pass.
  - bandwidth: each copy kind of 1 GiB or more (0.32's store and derived fills, 0.30's load and refill: record.py), as
    a share of the `ceiling` cell's measured ceiling, may not fall under 80% of its last passes' lowest share;
  - caps (`s<N>-<model>[-<i>]`, N GiB): `machine show` reports the cap on every GPU; the driver's bytes in
    use (Run.vram) never passed it; the kernels match `cold-<model>` (another kernel under pressure is a slow fallback);
    no bytes left behind; the third alternation costs at most CHURN more than the second;
  - host (`host-*`, a CPU rental): `host-cap` measured a cgroup cap below the RAM; each run kept its anonymous host
    high-water under the cap (its page/spill bytes are noted: a model that fits pages nothing);
  - every H3 cell produced its whole video;
  - no cell ends with varena backing that holds no page (resident - ready): a canceled prefetch's pages go back with it;
  - a graph served each cell of WARM. Falling back to eager is fine for a call and never an error, so nothing else would
    say that a model stopped using graphs: SDXL's steady step went from 0.08 s to 0.27 s with every rule and fault passing.
    check.py <gate dir> <SKU>     the same verdict alone"""
import json, os, re, subprocess, sys
from collections.abc import Iterator
from typing import Literal, TypeAlias, TypedDict, cast, overload

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..'))
import record  # noqa: E402  (release-gate/record.py: varena's own figures, 0.30's and 0.32's alike)
from runs import Baseline, GateHistory, GateRuns, KnownFailing, Memory, Run  # noqa: E402

GIB = 1 << 30
CHURN = 0.25
WARM = 'warm-sdxl',  # a warm call whose model has graphs on every card; known-failing.json parks one that can't
FLOOR, KEEP = 0.8, 5
CELL = re.compile(r's(\d+)-([a-z0-9]+)(?:-(\d+))?')
HERE = os.path.dirname(os.path.abspath(__file__))
Verdict: TypeAlias = tuple[list[str], list[str]]  # (fails, notes)
Ceiling: TypeAlias = dict[str, float]  # ceiling.py's document: disk_gbps, h2d_gbps when measured


class HostCap(TypedDict):  # host-cap.py's document
    cap: int | None
    mem_total: int | None
    swap_total: int | None


class VarenaGate(GateHistory, GateRuns):  # what check() reads of analyze.py's gate
    pass


class ShowGpu(TypedDict, total=False):  # `cozy machine show --json`: status.gpus[]
    memory_limit_bytes: int


class ShowStatus(TypedDict):
    gpus: list[ShowGpu]


class MachineShow(TypedDict):
    status: ShowStatus


def kernels(run: Run) -> tuple[list[list[str]], list[str]]:
    return run.gemm, run.attention


def memories(run: Run) -> list[Memory]:
    """The run's and its calls' memory records (H3 keeps its rows in its calls)."""
    return [memory for m in run.measurements if (memory := m.get('memory'))]


@overload
def result(run: Run | None, key: Literal['disk_gbps']) -> Ceiling | None: ...
@overload
def result(run: Run | None, key: Literal['cap']) -> HostCap | None: ...
def result(run: Run | None, key: str) -> Ceiling | HostCap | None:
    """The JSON document a script cell returned (a string field holding `key`)."""
    def walk(o: object) -> Iterator[object]:
        if isinstance(o, dict):
            for v in o.values():
                yield from walk(v)
        elif isinstance(o, list):
            for v in o:
                yield from walk(v)
        else:
            yield o
    return next((json.loads(v) for v in walk(run and run.result) if isinstance(v, str) and f'"{key}"' in v), None)


def bandwidth(recs: dict[str, Run], base: Baseline) -> Verdict:
    fails: list[str] = []
    notes: list[str] = []
    ceiling = result(recs.get('ceiling'), 'disk_gbps')
    if not ceiling:
        return fails, notes
    notes.append(f'ceiling: disk {ceiling["disk_gbps"]} GB/s' + (f', host to device {ceiling["h2d_gbps"]} GB/s' if ceiling.get('h2d_gbps') else ' (host to device unmeasured)'))
    hist = base.setdefault('varena', [])
    now: dict[str, float] = {}
    for name, run in recs.items():
        moved: dict[str, tuple[float, float]] = {}
        for m in memories(run):
            for kind, (b, s) in record.copies(m).items():
                mb, ms = moved.get(kind, (0, 0.0))
                moved[kind] = (mb + b, ms + s)
        for what, (b, s) in sorted(moved.items()):
            key = record.CEILING[what]
            if b < GIB or not s or not ceiling.get(key):
                continue
            share = b / s / 1e9 / ceiling[key]
            now[f'{name} {what}'] = round(share, 3)
            last = [h['shares'][f'{name} {what}'] for h in hist[-KEEP:] if f'{name} {what}' in h['shares']]
            line = f'{name} {what}: {b / GIB:.1f} GiB at {b / s / 1e9:.1f} GB/s, {share:.0%} of the ceiling'
            if last and share < FLOOR * min(last):
                fails.append(line + f' (last passes {min(last):.0%}-{max(last):.0%})')
            else:
                notes.append(line + (f' (last passes {min(last):.0%}-{max(last):.0%})' if last else ''))
    hist.append({'shares': now})
    del hist[:-KEEP]
    return fails, notes


def host(recs: dict[str, Run]) -> Verdict:
    fails: list[str] = []
    notes: list[str] = []
    cells = [n for n in recs if n.startswith('host-') and n != 'host-cap']
    if not cells and 'host-cap' not in recs:
        return fails, notes
    cap = result(recs.get('host-cap'), 'cap')
    # cast: past `not cap.get('cap') or not cap.get('mem_total')`, neither is None
    if not cap or not cap.get('cap') or not cap.get('mem_total') or cast(int, cap['cap']) >= cast(int, cap['mem_total']):
        return [f'host-cap: no host cap below the host RAM ({cap}): the host cells prove nothing here'], notes
    notes.append(f'host cap {cast(int, cap["cap"]) / GIB:.1f} GiB of {cast(int, cap["mem_total"]) / GIB:.1f} GiB RAM, swap {(cap.get("swap_total") or 0) / GIB:.1f} GiB')
    for name in cells:
        mems = memories(recs[name])
        moved = sum(record.paged(m) for m in mems)
        anon = max((h for m in mems if (h := record.host(m))), default=None)
        line = f'{name}: paged/spilled {moved / GIB:.1f} GiB' + (f', anonymous host high-water {anon / GIB:.1f} GiB' if anon else '')
        if anon and anon > cast(int, cap['cap']):
            fails.append(line + f' over the {cast(int, cap["cap"]) / GIB:.1f} GiB cap')
        else:
            notes.append(line)
    return fails, notes


def caps(out: str, recs: dict[str, Run]) -> Verdict:
    fails: list[str] = []
    notes: list[str] = []
    capped = [(n, int(m.group(1)), m.group(2), m.group(3)) for n in recs for m in [CELL.fullmatch(n)] if m]
    for gib in sorted({c[1] for c in capped}, reverse=True):
        show = os.path.join(out, f'cap-{gib}GiB.show.json')
        try:
            shown: MachineShow = json.load(open(show))
            limits = [gpu.get('memory_limit_bytes', 0) for gpu in shown['status']['gpus']]
        except (OSError, ValueError, KeyError) as error:
            fails.append(f'cap {gib} GiB: no machine show to read ({error})')
            continue
        if not limits or any(limit != gib * GIB for limit in limits):
            fails.append(f'cap {gib} GiB: machine show reports {limits}')
    execs: dict[str, int | None] = {}
    for name, gib, model, i in capped:
        run, mems = recs[name], memories(recs[name])
        moved = sum(record.paged(m) for m in mems)
        if run.vram:  # none read fails in analyze.py
            line = f'{name}: {run.vram["peak"] / 1024:.2f} GiB in use at its fullest (driver), streamed + paged {moved / GIB:.1f} GiB'
            (fails if run.vram['peak'] * 2**20 > gib * GIB else notes).append(line + f', cap {gib} GiB')
        left = sum(record.leftover(m) for m in mems)
        if left:
            fails.append(f'{name}: left {left / GIB:.2f} GiB behind')
        ref = recs.get(f'cold-{model}')
        if ref and model != 'h3' and kernels(ref) != kernels(run):
            fails.append(f'{name}: kernels {kernels(run)} under {gib} GiB; cold-{model} served {kernels(ref)}')
        execs[name] = run.execution_ms
    for model in sorted({m for _, _, m, i in capped if i and m != 'h3'}):
        runs = [cast(int, execs[n]) for n, _, m, i in sorted(capped, key=lambda c: int(c[3] or 0)) if m == model and i and execs.get(n)]  # execs.get(n) leaves no None
        if len(runs) >= 3:
            line = f'{model} alternation: ' + ' / '.join(f'{x / 1000:.1f}' for x in runs) + ' s'
            (fails if runs[2] > runs[1] * (1 + CHURN) + 5000 else notes).append(line)
    return fails, notes


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


def warm(recs: dict[str, Run]) -> Verdict:
    fails: list[str] = []
    notes: list[str] = []
    for name in WARM:
        run = recs.get(name)
        if run is None:  # analyze.py fails a cell that ran and didn't complete; check.py has no such cell to read
            continue
        if run.replays:
            notes.append(f'{name}: {run.replays} steps replayed')
        else:
            why = next((why[0] for why in run.safeguards.values()), 'no capture refused: nothing recorded it')
            fails.append(f'{name}: run {run.number} replayed no step, so its graphs are off: {why[:240]}')
    return fails, notes


def check(gate: VarenaGate) -> Verdict:
    recs, sku = gate['runs'], gate.get('sku', '')
    fails: list[str] = []
    notes: list[str] = []
    for f, n in (bandwidth(recs, gate.setdefault('baseline', {})), caps(gate['out'], recs), host(recs), videos(recs),
                 unowned(recs), warm(recs)):
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
