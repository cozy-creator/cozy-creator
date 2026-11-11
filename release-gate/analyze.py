"""Verdict for one release-gate directory. Each cell ran once on a fresh pod; its execution time, denoise step 1,
steady s/step and VRAM (the driver's bytes it added at its fullest: Run.vram) are held against the SKU's last known-good numbers (the baseline file,
gate-<SKU>.json, which gate.sh fetches from and publishes to the `release-gate-baselines` release). A PASS is recorded
in the baseline file; `--accept` records a FAIL the lead accepted. Exit 1 on any failure: the publish is blocked.
    analyze.py <gate dir> <SKU> <baseline file> [--accept]
More checks (quality, lm, swap, served, a suite's check.py) are modules imported here and listed in CHECKS: check(gate) -> (fails, notes)."""
import fnmatch, json, os, re, subprocess, sys, time
from collections.abc import Callable, Iterator
from typing import Literal, TypeAlias, TypedDict, cast
import numpy as np
from PIL import Image

out, sku, BASE = sys.argv[1].rstrip('/'), sys.argv[2], sys.argv[3]
accept = '--accept' in sys.argv
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, 'suites'))
import lm, quality, served, swap  # noqa: E402
from runs import JSON, STEP_EXTRA_MS, STEP_RATIO, Baseline, Cell, Gate, Good, KnownFailing, Observation, Run, slow_steps  # noqa: E402
from tensorfs import check as tensorfs  # noqa: E402  (release-gate/suites/tensorfs, never the tensorfs library)
from varena import check as varena  # noqa: E402
from cli import check as cli  # noqa: E402
from kernels import check as kernels  # noqa: E402
from attention import check as attention  # noqa: E402
from escaped import check as escaped  # noqa: E402
import bench  # noqa: E402  (release-gate/bench.py: suite bench's judging)
from contract import check as contract  # noqa: E402  (every gate: the records the other suites made)
# A suite's check runs when gate.sh ran the suite (--suite: the names in <gate dir>/suites).
SUITES = open(os.path.join(out, 'suites')).read().split() if os.path.exists(os.path.join(out, 'suites')) else []
Check: TypeAlias = Callable[[Gate], tuple[list[str], list[str]]]
SUITE_CHECKS: dict[str, Check] = {'tensorfs': tensorfs.check, 'varena': varena.check, 'cli': cli.check, 'kernels': kernels.check, 'attention': attention.check, 'escaped': escaped.check, 'bench': bench.check}  # runtime, packages: quality, lm and swap; hub, tensord: their expect rows
CHECKS: list[Check] = [quality.check, lm.check, swap.check, served.check, contract.check] + [SUITE_CHECKS[s] for s in SUITES if s in SUITE_CHECKS]
# Allowed rise over the worst known-good number, unless the good numbers' own spread is wider.
# Step 1 is judged as load + step 1 (the run's or call's start to step 1's end; agreed by the lead, 2026-11-06): v2 readies
# weights on first use inside step 1, where v1 readied them in a load before it. step1_ms is kept, not judged.
Judged: TypeAlias = Literal['execution_ms', 'to_step1_ms', 'steady_ms', 'vram_mb']
SLACK: dict[Judged, float] = {'execution_ms': 0.25, 'to_step1_ms': 0.5, 'steady_ms': 0.10, 'vram_mb': 0.10}
# And at least this much: a CUDA context's size moves between drivers (MiB); a sub-second CPU cell's time is its
# host's scheduling (ms).
FLOOR = {'vram_mb': 256, 'execution_ms': 250}
# Request-path blocking work the Runtime still does (tracker cr-217), as fnmatch patterns. The list may only shrink.
KNOWN_BLOCKING = os.path.join(HERE, 'known-blocking.json')
BLOCKING_SLACK = 0.5
# Cells that fail on a known open bug and are not run until it is fixed (lead-ruled), listed in every verdict.
KNOWN_FAILING = os.path.join(HERE, 'known-failing.json')
# The host's own speed: each executor's package import (its CPU) and CUDA init, held against the same cells' known-good.
# At SLOW_HOST times those or more, the host is slow and its time fails say so, not a regression (purin 11-05: imports
# 2.6x and CUDA init 2.2x mochizou's, th-364). A leg under HOST_FLOOR_MS (an executor forked from an imported parent) isn't judged.
HOST_LEGS, SLOW_HOST, HOST_FLOOR_MS = ('package_import', 'cuda_init'), 2, 250
fails: list[str]
notes: list[str]
fails, notes = [], []


class KnownBlocking(TypedDict):
    pattern: str  # fnmatch, over the blocking work's name
    issue: str
    site: str


class KernelProbe(TypedDict, total=False):
    """A kernel self-check in machine-end.json's `status.kernels`."""
    ok: bool
    kernel: str
    sm: int
    driver: str
    detail: str


def walk(o: JSON) -> Iterator[dict[str, JSON]]:
    if isinstance(o, dict):
        yield o
        for v in o.values():
            yield from walk(v)
    elif isinstance(o, list):
        for v in o:
            yield from walk(v)


# A function renamed by a release, so its cells hold against the numbers the old name earned (xs-139: 0.3.0's generate
# is 0.2.x's shot).
RENAMED = {'minimax-h3/shot': 'minimax-h3/generate'}


def canonical(target: str) -> str:
    """The package function a target names, whether published (org/pkg/fn@v) or a checkout (./pkg/fn)."""
    name = re.sub(r'^(\./|[^/]+/(?=[^/]+/[^/]+$))', '', re.sub(r'@.*$', '', target))
    return RENAMED.get(name, name)


# A device without fp8 tensor cores (before sm89) serves an fp8 lane's weights at 16-bit compute (cozy-creator #2210):
# the expected path there, not a fallback.
def fp8_weight_only(o: Observation) -> bool:
    f = o.get('fields') or {}
    m = re.search(r'needs cuda\.sm(\d+)\+ and this device is cuda\.sm(\d+)', cast(str, f.get('reason') or ''))  # its reason is text
    return (o.get('name') == 'gemm.fallback' and f.get('wanted') == 'fp8' and f.get('served') in ('bf16', 'fp16')
            and m is not None and int(m.group(2)) < 89 <= int(m.group(1)))


cells = [l.rstrip('\n').split('\t') for l in open(os.path.join(out, 'cells.tsv')) if l.strip()]
summary: dict[str, Cell]
c: Cell | None
runs: dict[str, Run]
host_legs: dict[str, dict[str, int]]
summary, runs, host_legs = {}, {}, {}
for name, target, number in cells:
    run = Run.read(os.path.join(out, name + '.json'))
    if run is None or run.status != 'completed':
        fails.append(f'{name}: run {number} {run and run.status} {run and run.reason or None}')
        continue
    runs[name] = run
    # A run completes even when the test it ran failed: its own verdict is the cell's.
    if isinstance(run.result, dict) and run.result.get('passed') is False:
        fails.append(f'{name}: run {number} says passed: false: {str(run.result.get("detail"))[-400:]}')
    obs, gemm, attn = run.observations, run.gemm, run.attention
    for off in run.capabilities_off:
        fails.append(f'{name}: the Runtime turned off {off}')
    # A graph that differed from eager was caught, and is still a bug. A refusal only costs speed: check.py judges that.
    for code, why in run.safeguards.items():
        (fails if code == 'replay_mismatch' else notes).append(f'{name}: {code}: {why[0][:200]} ({len(why)})')
    for o in obs:
        text = f"{o.get('name')} {o.get('value')}"
        if fp8_weight_only(o):
            f = o['fields']
            notes.append(f'{name}: fp8 weights at {f["served"]} compute ({f.get("component")}, {f.get("sites")} sites): {f["reason"]}')
        elif any(s in text for s in ('gemm.fallback', 'decoded_float')):
            fails.append(f'{name}: {text} {o.get("fields")}')
    for fmt, val in gemm:
        if fmt.startswith('int8') and val != 'cozy-int8':
            fails.append(f'{name}: int8 GEMM served by {val}, not cozy-int8')
        if any(s in val for s in ('_int_mm', 'dense', 'decoded_float', 'fallback')):
            fails.append(f'{name}: GEMM {fmt} served by {val}')
    blocking: dict[str, int] = {}
    for o in obs:
        if o.get('name') == 'request_path.blocking':  # its value names the work, its `ms` field is a number
            blocking[cast(str, o['value'])] = blocking.get(cast(str, o['value']), 0) + round(float(cast(float, o['fields'].get('ms', 0))))
    s = run.steps
    legs: dict[str, int] = {}
    for m in run.measurements:
        ex = m.get('execution') or {}
        part: Literal['executor', 'construction']
        for part in ('executor', 'construction'):
            for leg in ('cuda_init', 'measure_device', 'package_import'):
                v = ((ex.get(part) or {}).get('legs_ms') or {}).get(leg)
                if v: legs[leg] = max(legs.get(leg, 0), round(v))
    host_legs[name] = legs
    vram = run.vram
    if vram:
        notes.append(f'{name} VRAM: {vram["added"] / 1024:.1f} GiB added, {vram["peak"] / 1024:.1f} GiB in use at its fullest (driver)')
    elif run.gpus:
        fails.append(f'{name}: run {run.number} has no driver VRAM reading (memory.driver): its machine\'s TensorD predates it')
    # Step 1 is judged without the work of getting its weights ready (a fresh pod's cold disk, a LoRA's bake): execution time keeps it.
    reads = run.step1_read_ms
    if s and reads:
        notes.append(f'{name}: step 1 {s[0] / 1000:.1f} s, of which {reads / 1000:.1f} s readying weights (judged in execution time)')
        s = [s[0] - reads] + s[1:]
    c = summary[name] = {'blocking': blocking, 'run': run.number, 'target': canonical(target), 'gemm': gemm, 'attention': attn, 'graphs': run.graphs,
                         'execution_ms': run.execution_ms, 'step1_ms': round(s[0]) if s else None, 'step1_read_ms': round(reads), 'to_step1_ms': run.to_step1_ms,
                         'vram_mb': vram and vram['added'], 'host': {leg: legs[leg] for leg in HOST_LEGS if leg in legs},
                         'steady_ms': round(sum(s[1:]) / len(s[1:])) if len(s) > 1 else None, 'files': list(run.outputs)}
    for i, ms, ref, what in slow_steps(canonical(target), s):
        if i == 1:
            continue  # step 1 is judged as load + step 1 against the known-good (to_step1_ms)
        fails.append(f'{name}: step {i} {ms / 1000:.1f} s vs {what} {ref / 1000:.1f} s (at most {STEP_RATIO}x + {STEP_EXTRA_MS // 1000} s)')


def image(name: str) -> Image.Image | None:
    c = summary.get(name)
    if not c: return None
    p = [f for f in c['files'] if f.endswith(('.png', '.webp', '.jpg'))]
    if not p:
        fails.append(f'{name}: no image output'); return None
    im = Image.open(p[0]); a = np.asarray(im.convert('RGB')).astype(float)
    if a.std() < 10: fails.append(f'{name}: flat image (std {a.std():.1f})')
    return im


for n in ('sdxl', 'klein', 'anima', 'sdxl-p2'):
    image(n)
# Candidate cells: any other image output must not be flat.
for n, c in summary.items():
    if n not in ('sdxl', 'klein', 'anima', 'sdxl-p2', 'qwen', 'birefnet', 'describer', 'h3', 'seedvr2') and any(f.endswith(('.png', '.webp', '.jpg')) for f in c['files']):
        image(n)
im = image('qwen')
if im is not None:
    if im.mode != 'RGBA':
        fails.append(f'qwen: {im.mode}, not RGBA')
    else:
        z = (np.asarray(im)[..., 3] == 0).mean(); notes.append(f'qwen: alpha 0 {z:.0%}')
        if z < 0.05: fails.append(f'qwen: no real transparency (alpha 0 {z:.1%})')
im = image('birefnet')  # composites the cutout onto white
if im is not None:
    white = (np.asarray(im.convert('L')) > 245).mean(); notes.append(f'birefnet: white background {white:.0%}')
    if white < 0.05: fails.append(f'birefnet: no white background ({white:.1%})')
if 'describer' in runs:
    texts = [s for d in walk(runs['describer'].result) for v in d.values() for s in (v if isinstance(v, list) else [v])
             if isinstance(s, str) and len(s) > 20]
    if not texts: fails.append('describer: no description text')
for n, min_h, min_frames in (('h3', 700, 300), ('seedvr2', 1000, 20)):
    c = summary.get(n)
    vids = [f for f in (c['files'] if c else []) if f.endswith('.mp4')]
    if c and not vids:
        fails.append(f'{n}: no video output'); continue
    if vids:
        p = subprocess.run(['ffprobe', '-v', 'error', '-select_streams', 'v:0', '-count_packets', '-show_entries',
                            'stream=height,nb_read_packets', '-of', 'csv=p=0', vids[-1]], capture_output=True, text=True)
        h, fr = (int(x) for x in p.stdout.strip().split(',')[:2])
        notes.append(f'{n}: {h}p, {fr} frames')
        if h < min_h or fr < min_frames: fails.append(f'{n}: video {h}p, {fr} frames')
a, b = summary.get('sdxl'), summary.get('sdxl-p2')
if a and b:
    if not b['gemm'] and not b['attention']:
        fails.append('sdxl-p2: no kernel observations (memoized or not executed)')
    elif (a['gemm'], a['attention']) != (b['gemm'], b['attention']):
        fails.append(f'sdxl-p2: second executor serves {b["gemm"]} / {b["attention"]}, first {a["gemm"]} / {a["attention"]}')

# Against the last known-good numbers.


def against(mine: int | None, theirs: int | None) -> str:
    return f'{mine} ms vs {theirs} ms ({(mine - theirs) / theirs:+.1%})' if mine and theirs else 'not measured'


# released.sh's pairs: a checkout cell and the same request and seed from the release the Hub serves, on the same pod.
for name in [n for n in summary if n.endswith(('-released', '-released-again'))]:
    mine = name.replace('-released', '', 1)
    if mine not in summary:
        continue
    a, b = summary[mine], summary[name]
    digests = [sorted(o.get('sha256', '') for o in runs[n].record.get('output') or []) for n in (mine, name)]
    notes.append(f'{mine} vs the release ({name}): execution {against(a["execution_ms"], b["execution_ms"])}, steady'
                 f' {against(a["steady_ms"], b["steady_ms"])}, output {"identical" if digests[0] == digests[1] else "differs"}')

base: Baseline = json.load(open(BASE)) if os.path.exists(BASE) else {'sku': sku, 'cells': {}}
known: list[KnownBlocking] = json.load(open(KNOWN_BLOCKING)) if os.path.exists(KNOWN_BLOCKING) else []
mid: Callable[[list[float]], float] = lambda xs: sorted(xs)[len(xs) // 2]
known_good: Callable[[str, Cell], list[Good]] = lambda name, c: [g for g in base['cells'].get(name, {}).get('good', []) if canonical(g['target']) == c['target']]
slow: list[str] = []
for leg in HOST_LEGS:
    pairs: list[tuple[int, int]] = []  # (this run's, the most its known-good took)
    for name, c in summary.items():
        vals = [g['host'][leg] for g in known_good(name, c) if (g.get('host') or {}).get(leg, 0) >= HOST_FLOOR_MS]
        if vals and c['host'].get(leg, 0) >= HOST_FLOOR_MS:
            pairs.append((c['host'][leg], max(vals)))
    if not pairs:
        continue
    ratio = mid([now / good for now, good in pairs])
    notes.append(f'host {leg}: median {mid([p[0] for p in pairs]) / 1000:.1f} s vs known-good {mid([p[1] for p in pairs]) / 1000:.1f} s,'
                 f' {ratio:.1f}x over {len(pairs)} cells')
    if ratio >= SLOW_HOST:
        slow.append(f'{leg} {ratio:.1f}x')
if slow:
    notes.append(f'slow host: {" and ".join(slow)} its cells\' known-good: its time limits judge the host, not the code')
for name, c in summary.items():
    good = known_good(name, c)
    before = good[-1].get('blocking', {}) if good else {}
    for work, ms in sorted(c['blocking'].items()):
        site = next((k for k in known if fnmatch.fnmatch(work, k['pattern'])), None)
        if site is None:
            fails.append(f'{name}: NEW request-path blocking: {work} {ms / 1000:.1f} s')
        elif work in before and ms > before[work] * (1 + BLOCKING_SLACK) + 1000:
            fails.append(f'{name}: known blocking {work} grew {before[work] / 1000:.1f} -> {ms / 1000:.1f} s ({site["issue"]})')
        else:
            notes.append(f'{name}: known blocking {work} {ms / 1000:.1f} s ({site["issue"]}, {site["site"]})')
for name, c in summary.items():
    good = known_good(name, c)
    if not good:
        notes.append(f'{name}: no known-good numbers for {c["target"]} on {sku}: this run starts them'); continue
    last = good[-1]
    if (last['gemm'], last['attention']) != (c['gemm'], c['attention']):
        fails.append(f'{name}: kernels {last["gemm"]} / {last["attention"]} -> {c["gemm"]} / {c["attention"]}')
    if last.get('graphs') == 'native' and c['graphs'] == 'eager':
        fails.append(f'{name}: CUDA graphs native -> eager (its denoise ran no captured step)')
    for k, slack in SLACK.items():
        vals = [cast(int, g[k]) for g in good if g.get(k) is not None]  # not None: filtered
        if c.get(k) is None:
            continue
        if not vals:
            notes.append(f'{name}: no known-good {k} for {c["target"]} on {sku}: this run starts it'); continue
        limit = max(vals) + max(slack * max(vals), max(vals) - min(vals), FLOOR.get(k, 0))
        unit, div = ('GiB', 1024) if k == 'vram_mb' else ('s', 1000)
        # c[k] is not None: skipped above.
        line = (f'{name} {k.rsplit("_", 1)[0]}: {cast(int, c[k]) / div:.1f} {unit} vs known-good {min(vals) / div:.1f}-{max(vals) / div:.1f} {unit}'
                f' (limit {limit / div:.1f})')
        (fails if cast(int, c[k]) > limit else notes).append(line + (' (slow host)' if slow and cast(int, c[k]) > limit and k != 'vram_mb' else ''))

failing: KnownFailing
for failing in (json.load(open(KNOWN_FAILING)) if os.path.exists(KNOWN_FAILING) else []):
    if failing['sku'] in (sku, '*'):
        notes.append(f"known failing, not run: {failing['cell']} ({failing['issue']}: {failing['what']})")
# The host: a slow driver makes every first pass slow and must read as the host, not the code (lead 10-23).
for leg in ('cuda_init', 'measure_device'):
    vals = [l[leg] for l in host_legs.values() if leg in l]
    if vals:
        notes.append(f'host {leg}: median {sorted(vals)[len(vals) // 2] / 1000:.1f} s, max {max(vals) / 1000:.1f} s over {len(vals)} executors')
# A GPU fault, or a kernel self-check that crashed its process, fails the gate even when every cell passed: a group walked
# into a faulting kernel on corteo with only the machine's record saying so. The machine's own log and record, kept by end_it.
LOG = os.path.join(out, 'machine-tensord.log')
# The page faults tool_guard reports it caused, on purpose and off every run, are the only ones forgiven.
SUMMARY = os.path.join(out, 'faults', 'out', 'summary.txt')
meant = sum(len(json.loads(l)['xids']) for l in open(SUMMARY) if json.loads(l)['cell'] == 'tool_guard') if os.path.exists(SUMMARY) else 0
for line in (l for l in open(LOG, errors='replace') if re.search(r'GPU \d+ (reported Xid|was lost).*: failing', l)) if os.path.exists(LOG) else ():
    if meant and 'Xid 31' in line:
        meant -= 1
    else:
        fails.append(f'gpu fault: {line.strip()[:200]}')
END = os.path.join(out, 'machine-end.json')
kernel: KernelProbe
for kernel in (json.load(open(END)).get('status', {}).get('kernels', []) if os.path.getsize(END) else []) if os.path.exists(END) else []:
    if not kernel.get('ok'):
        fails.append(f"kernel probe crashed: {kernel.get('kernel')} sm{kernel.get('sm')} driver {kernel.get('driver')}: {kernel.get('detail', '')[:160]}")
gate: Gate = {'out': out, 'sku': sku, 'summary': summary, 'runs': runs, 'baseline': base}
# CLI-behaviour checks a cells/*.sh recorded with gate.sh's `expect` (expect.tsv: check, PASS|FAIL, what happened).
EXPECT = os.path.join(out, 'expect.tsv')
for row in (l.rstrip('\n').split('\t') + [''] for l in open(EXPECT)) if os.path.exists(EXPECT) else []:
    (notes if row[1] == 'PASS' else fails).append(f'{row[0]}: {row[1]} {row[2]}'.rstrip())
for check in CHECKS:
    found, noted = check(gate)
    fails += found; notes += noted
for n in notes: print('note', n)
for line in fails: print('FAIL', line)
print('SUITES', ','.join(SUITES) or 'none')
print('VERDICT', 'FAIL' if fails else 'PASS', f'({len(cells)} cells)', '' if not fails or not accept else '(accepted by the lead)',
      f'(slow host: {" and ".join(slow)})' if slow and fails else '')
if not fails or accept:
    label = open(os.path.join(out, 'label')).read().strip()
    for name, c in summary.items():
        entry: Good = {'run': c['run'], 'target': c['target'], 'gemm': c['gemm'], 'attention': c['attention'], 'graphs': c['graphs'],
                       'execution_ms': c['execution_ms'], 'step1_ms': c['step1_ms'], 'to_step1_ms': c['to_step1_ms'],
                       'steady_ms': c['steady_ms'], 'vram_mb': c['vram_mb'], 'blocking': c['blocking'], 'host': c['host']}
        entry.update({'label': label, 'at': time.strftime('%Y-%m-%dT%H:%MZ', time.gmtime())})
        base['cells'].setdefault(name, {'good': []})['good'].append(entry)
    json.dump(base, open(BASE, 'w'), indent=1)
    print('recorded', BASE)
sys.exit(1 if fails else 0)
