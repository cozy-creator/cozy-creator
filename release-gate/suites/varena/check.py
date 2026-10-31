"""The varena suite's verdict (cells.sh), one of analyze.py's CHECKS; analyze.py already holds every cell to completion,
kernels, time and VRAM against the last pass.
  - bandwidth: each copy kind of 1 GiB or more (0.32's store and derived fills, 0.30's load and refill: record.py), as
    a share of the `ceiling` cell's measured ceiling, may not fall under 80% of its last passes' lowest share;
  - caps (`s<N>-<model>[-<i>]`, N GiB): the ledger took the cap (where the memory log names it); the driver's bytes in
    use (Run.vram) never passed it; the kernels match `cold-<model>` (another kernel under pressure is a slow fallback);
    no bytes left behind; the third alternation costs at most CHURN more than the second;
  - host (`host-*`, a CPU rental): `host-cap` measured a cgroup cap below the RAM; each run kept its anonymous host
    high-water under the cap (its page/spill bytes are noted: a model that fits pages nothing);
  - every H3 cell produced its whole video;
  - no cell ends with varena backing that holds no page (resident - ready): a canceled prefetch's pages go back with it.
    check.py <gate dir> <SKU>     the same verdict alone"""
import json, os, re, subprocess, sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..'))
import record  # noqa: E402  (release-gate/record.py: varena's own figures, 0.30's and 0.32's alike)
from runs import Run  # noqa: E402

GIB = 1 << 30
CHURN = 0.25
FLOOR, KEEP = 0.8, 5
CELL = re.compile(r's(\d+)-([a-z0-9]+)(?:-(\d+))?')
HERE = os.path.dirname(os.path.abspath(__file__))


def kernels(run):
    return run.gemm, run.attention


def memories(run):
    """The run's and its calls' memory records (H3 keeps its rows in its calls)."""
    return [m['memory'] for m in run.measurements if m.get('memory')]


def result(run, key):
    """The JSON document a script cell returned (a string field holding `key`)."""
    def walk(o):
        if isinstance(o, dict):
            for v in o.values():
                yield from walk(v)
        elif isinstance(o, list):
            for v in o:
                yield from walk(v)
        else:
            yield o
    return next((json.loads(v) for v in walk(run and run.result) if isinstance(v, str) and f'"{key}"' in v), None)


def bandwidth(recs, base):
    fails, notes = [], []
    ceiling = result(recs.get('ceiling'), 'disk_gbps')
    if not ceiling:
        return fails, notes
    notes.append(f'ceiling: disk {ceiling["disk_gbps"]} GB/s' + (f', host to device {ceiling["h2d_gbps"]} GB/s' if ceiling.get('h2d_gbps') else ' (host to device unmeasured)'))
    hist = base.setdefault('varena', [])
    now = {}
    for name, run in recs.items():
        moved = {}
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


def host(recs):
    fails, notes = [], []
    cells = [n for n in recs if n.startswith('host-') and n != 'host-cap']
    if not cells and 'host-cap' not in recs:
        return fails, notes
    cap = result(recs.get('host-cap'), 'cap')
    if not cap or not cap.get('cap') or not cap.get('mem_total') or cap['cap'] >= cap['mem_total']:
        return [f'host-cap: no host cap below the host RAM ({cap}): the host cells prove nothing here'], notes
    notes.append(f'host cap {cap["cap"] / GIB:.1f} GiB of {cap["mem_total"] / GIB:.1f} GiB RAM, swap {(cap.get("swap_total") or 0) / GIB:.1f} GiB')
    for name in cells:
        mems = memories(recs[name])
        moved = sum(record.paged(m) for m in mems)
        anon = max((h for m in mems if (h := record.host(m))), default=None)
        line = f'{name}: paged/spilled {moved / GIB:.1f} GiB' + (f', anonymous host high-water {anon / GIB:.1f} GiB' if anon else '')
        if anon and anon > cap['cap']:
            fails.append(line + f' over the {cap["cap"] / GIB:.1f} GiB cap')
        else:
            notes.append(line)
    return fails, notes


def caps(out, recs):
    fails, notes = [], []
    capped = [(n, int(m.group(1)), m.group(2), m.group(3)) for n in recs for m in [CELL.fullmatch(n)] if m]
    for gib in sorted({c[1] for c in capped}, reverse=True):
        log = os.path.join(out, f'cap-{gib}GiB.memory.log')
        ledger = [int(x) for x in re.findall(r'"capacity":(\d+)[^\n]*"event":"ledger"', open(log).read())] if os.path.exists(log) else []
        if not ledger:  # TensorD 0.11.17's memory log names no ledger capacity; what each run held is the check
            notes.append(f'cap {gib} GiB: the memory log names no ledger capacity')
        elif ledger[-1] > gib * GIB:
            fails.append(f'cap {gib} GiB: the ledger took {ledger[-1] / GIB:.2f} GiB')
    execs = {}
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
        runs = [execs[n] for n, _, m, i in sorted(capped, key=lambda c: int(c[3] or 0)) if m == model and i and execs.get(n)]
        if len(runs) >= 3:
            line = f'{model} alternation: ' + ' / '.join(f'{x / 1000:.1f}' for x in runs) + ' s'
            (fails if runs[2] > runs[1] * (1 + CHURN) + 5000 else notes).append(line)
    return fails, notes


def videos(recs):
    fails, notes = [], []
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


def unowned(recs):
    fails = []
    for name, run in recs.items():
        left = sum(record.unowned(m) for m in memories(run))
        if left:
            fails.append(f'{name}: {left / 2**20:.0f} MiB of varena backing holds no page at its end (resident - ready)')
    return fails, []


def check(gate):
    recs, sku = gate['runs'], gate.get('sku', '')
    fails, notes = [], []
    for f, n in (bandwidth(recs, gate.setdefault('baseline', {})), caps(gate['out'], recs), host(recs), videos(recs),
                 unowned(recs)):
        fails += f; notes += n
    parked = [c for k in json.load(open(os.path.join(HERE, '..', '..', 'known-failing.json')))
              if k['sku'] in (sku, '*') for c in k.get('cells', [])]
    if parked:
        notes.append(f'varena: parked on {sku}: {", ".join(parked)}')
    return fails, notes


if __name__ == '__main__':
    out, sku = sys.argv[1].rstrip('/'), sys.argv[2]
    runs = {}
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
