"""The fixed benchmark suite's numbers, stored baselines and table (suites/bench/README.md).
    bench.py table <gate dir>... [--baseline FILE] [--cells FILE] [--known FILE] [--refs DIR] [--no-output]
        each group's cells against the stored baseline and ComfyUI; exit 1 on any FAIL
    bench.py versus <gate dir>...   each cell with a ComfyUI row against it: warm total, steady step, peak VRAM, cold, and
        H3 by stage; every cell where Cozy is slower or holds more memory, with the gap
    bench.py baseline <version> <gate dir>... [--out FILE] [--refs DIR] [--comfyui-out FILE] [--cells FILE]
        baselines/<version>.json from those dirs, Cozy's numbers only: ComfyUI's rows go to --comfyui-out, kept off the public
        repo until every cell is won (Paul 11-10); each cell's reference output copied into --refs (baselines/refs/,
        untracked: assets of the release-gate-baselines release, which `table` fetches). `table --comfyui FILE` reads them back
    bench.py admit <gate dir> [--baseline FILE]
        the pod's reference workload against its GPU type's stored one; exit 1 when out of range
    bench.py noise <gate dir>...     each cell's run-to-run spread over its warm runs
    bench.py chart <gate dir>... [--out FILE] [--title T]   the ComfyUI comparison as an SVG grid (model x GPU)
    bench.py scaling <gate dir>...   each multi-GPU cell against its one-GPU cell: transport, dense and sparse steps, speed-up
A gate dir is gate.sh's --out after suite `bench` (suites/bench/cells.sh): bench.tsv names each cell's runs
(cell, cold|warm, record, the VRAM holder's MiB), bench-group its group (<sku>x<gpus>), calibrate-{before,after}.json the reference workload.
Outputs are judged with numpy and ffmpeg: uv run -q --no-project --with numpy python -I bench.py ..."""
import glob
import json
import math
import os
import re
import shutil
import statistics
import subprocess
import sys
from collections.abc import Callable
from dataclasses import dataclass, field
from functools import partial
from typing import Literal, NotRequired, TypedDict, cast

HERE = os.path.dirname(os.path.abspath(__file__))
BENCH = os.path.join(HERE, 'suites', 'bench')
sys.path.insert(0, HERE)
sys.path.insert(0, BENCH)
import media  # noqa: E402  (suites/bench/media.py)
from comfy import Row as Comfy, Tier  # noqa: E402  (suites/bench/comfy.py: ComfyUI's row for a cell)
from runs import Run  # noqa: E402

# The owner's bar: a judged time more than 5% over its baseline fails, unless the cell's own measured noise is wider.
BAR = 0.05
# One warm run's relative standard deviation on one pod, at least (compute-bound rows, 30 repeats on kesshouban and
# pasan, 11-07..09: median 0.8%), and between two pods of one GPU type: 1.4% once each call's time is scaled by its own
# mean SM clock (same build, 3 pods), 3.2% unscaled. A capped cell pages weights over PCIe, which the SM clock does not
# pace: it is judged unscaled.
SD_FLOOR, POD_SD, POD_SD_UNSCALED = 0.008, 0.014, 0.032
D2 = {2: 1.128, 3: 1.693, 4: 2.059, 5: 2.326}  # the expected range of n normal draws, in standard deviations
Judged = Literal['steady_ms', 'total_ms']
JUDGED: tuple[Judged, ...] = ('steady_ms', 'total_ms')
TOTAL_FLOOR_MS = 300  # a total must also be this much worse: sub-second cells are their host's scheduling
COLD_NOTE = 0.25  # a cold total this far over its baseline's is noted: one sample, and the pod's disk sets it
# A reference workload this far from its GPU type's stored one is another class of pod. GPU kernels: kesshouban to pasan
# 7%. Host CPU: normal 4090 hosts (canute, lieri, kesshouban, crystal) span fork 117-149 ms, cuda_init 0.79-0.88 s, package
# import 2.0-2.2 s, tokenize 1.4-1.7 ms; the slow class (euclase, eleonora, norgalle, 11-10) is 2-4x, and steps eager SDXL at
# 193 ms against 120 (capped-sdxl.md).
POD_RANGE = {'attention_ms': 0.12, 'conv_ms': 0.12, 'gemm_ms': 0.12, 'h2d_gbps': 0.35, 'disk_gbps': 0.6,
             'python_ms': 0.5, 'launch_us': 0.5, 'tokenize_ms': 0.5, 'exec_fork_ms': 0.75, 'cuda_init_ms': 0.5}
# Whatever is stored: a host whose executor took this long is the slow class (its record's legs, TensorD's own reads).
# CUDA init grows with the GPUs it opens: five 2x 4090 hosts in a row read 1545-1616 ms (11-10), the 1x slow class 1.7-2.6 s.
HOST_CEILING = {'exec_fork_ms': 220.0, 'cuda_init_ms': 1300.0}
CUDA_INIT_PER_GPU = 400.0
DISK_STALL_S = 10.0  # pod.sh disk: jeanne's NVMe completed nothing for 25-30 s at a time (11-11)
SLOW_LEG = 1.5  # a cell whose executor's package import or CUDA init is this much over its baseline's: noted
DRIFT = 0.03  # the reference workload after the cells against before them
REPO, TAG = 'cozy-creator/cozy-creator', 'release-gate-baselines'
MEDIA = ('video', 'image', 'audio')  # a run's judged output: its first file of the first of these kinds it has
NUMBERS = ('total_ms', 'execution_ms', 'step1_ms', 'to_step1_ms', 'steady_ms', 'denoise_ms', 'steps', 'vram_mib', 'proc_mib', 'host_low_mib',
           'fills', 'evictions', 'refilled_gib', 'replays', 'clock_mhz', 'clock_limit_mhz', 'throttled', 'temperature_c', 'power_w',
           'fork_ms', 'cuda_init_ms', 'package_import_ms')


class Transport(TypedDict):
    route: str | None
    degree: int | None
    mapped: bool | None
    exact: bool | None


class Kernels(TypedDict):
    gemm: list[list[str]]
    attention: list[str]
    graphs: str | None


class Output(TypedDict):
    sha256: str
    seed: int | None
    faults: list[str]
    file: NotRequired[str]  # this gate's copy
    asset: NotRequired[str]  # a stored reference's name among the release-gate-baselines assets
    look: NotRequired[media.Look]


class Metrics(TypedDict):
    """One run."""
    run: int | None
    status: str
    reason: str
    seed: int | None
    memo: list[str]  # what the machine's memo answered: `run` (all of it) or the calls
    computed: bool  # False when the memo answered all of it
    numbers: dict[str, float]  # NUMBERS that were read
    kernels: Kernels
    transport: Transport | None
    throttles: list[str]
    output: Output | None
    stages: dict[str, float]  # the record's named stages, ms (attribution, the run's and its calls')
    series: list[float]  # the dominant sampler track's steps, ms
    build: str  # cells.sh: this computer's cozy that sent it (version@digest, `a>b` when it changed meanwhile), then the machine's


class Cell(TypedDict):
    """A cell: its cold run (the cell's fixed seed, whose output is the judged one) and its warm runs' medians."""
    cold: Metrics | None
    warm: dict[str, float]
    spread: dict[str, float]  # one warm run's relative standard deviation, from their range
    n: int
    runs: list[int | None]
    output: Output | None
    failed: list[str]  # runs that did not complete, or measured nothing
    faults: list[str]  # outputs that are no output
    kernels: Kernels | None
    transport: Transport | None
    throttles: list[str]
    stages: dict[str, float]  # the warm runs' median of each stage, ms
    series: list[float]  # the warm runs' median of each step, ms
    builds: list[str]  # every machine build (cozy, Runtime, TensorFS) its runs ran on
    senders: list[str]  # every build of this computer's cozy that sent them (the RC laptop rebuilds live)
    cold_downloaded: bool  # its weights step made no run (a cozy without --warm), so its cold run fetched them


class Reference(TypedDict, total=False):
    """The pod's reference workload (suites/bench/calibrate), its record's GPU reads and legs, and pod.sh `facts`."""
    gpu: str  # pod.sh facts (pod-facts.txt)
    driver: str
    pcie: str
    power_limit_w: float
    cpu: str
    cores: int
    cpu_limit: float | None
    python_ms: float
    launch_us: float
    tokenize_ms: float
    exec_fork_ms: float  # the calibrate executor's own legs, from its record
    cuda_init_ms: float
    ctx_mib: float  # an executor's CUDA context: the calibrate call's driver peak less its process's, alone on the GPU
    host_gib: float
    limit_gib: float | None
    disk_stall_s: float  # pod.sh disk: the longest any disk held requests in flight and completed none
    disk_read_gbps: float
    clock_mhz: float  # TensorD's reads of the calibrate call: its SM clock and hottest temperature
    temperature_c: float
    attention_ms: float
    conv_ms: float
    gemm_ms: float
    h2d_gbps: float
    disk_gbps: float


class Gate(TypedDict):
    dir: str
    group: str
    pod: str | None
    label: str | None
    host: str | None  # cells.sh's host-ram: the RAM the pod was held to, or that it was not held and why
    reference: dict[str, Reference]  # before, after
    cells: dict[str, Cell]
    comfyui: dict[str, Comfy]  # measured on this pod after the cells (comfyui.sh), when it ran
    invalid: str | None  # a gate dir's `invalid` file: why its numbers don't count (a pod fault found after it ran)


class Group(TypedDict):
    pod: str | None
    label: str | None
    host: NotRequired[str | None]
    reference: Reference
    cells: dict[str, Cell]
    comfyui: dict[str, Comfy]


class Baseline(TypedDict):
    version: str
    groups: dict[str, Group]


class CellDef(TypedDict):
    """A row of suites/bench/cells.tsv (`-` is empty)."""
    name: str
    group: str
    cap: str  # GiB
    warm: str
    seed: str
    target: str
    args: str
    before: str
    beats: str
    like: str
    route: str
    comfyui: str  # the benchmarks runner's cell of the same request (comfy.py)
    guards: str


class Known(TypedDict):
    cell: str
    issue: str
    reason: str
    group: NotRequired[str]


@dataclass
class Verdict:
    name: str
    base: Cell | None
    cand: Cell | None
    comfy: Comfy | None
    verdict: str = 'PASS'
    why: list[str] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)
    ratios: dict[str, tuple[float, float, float]] = field(default_factory=dict)  # raw, scaled, limit


def in_group(pattern: str, group: str) -> bool:
    """A cells.tsv `group`: a group, or comma-separated `*<suffix>` patterns (`*.matrix`: every matrix group)."""
    return any(p == group or (p.startswith('*') and group.endswith(p[1:])) for p in pattern.split(','))


def load_json(path: str) -> object:
    with open(path) as f:
        return cast(object, json.load(f))  # json.load is untyped; each caller narrows what it reads


def transport_of(o: object) -> Transport | None:
    """The first `transport` with a route anywhere in the record (a group's exchange)."""
    if isinstance(o, dict):
        t = o.get('transport')
        if isinstance(t, dict) and t.get('route'):
            check, degree = t.get('check'), t.get('degree')
            mapped, exact = (check.get('mapped'), check.get('exact')) if isinstance(check, dict) else (None, None)
            here: Transport = {'route': str(t['route']), 'degree': degree if isinstance(degree, int) else None,
                                'mapped': mapped if isinstance(mapped, bool) else None, 'exact': exact if isinstance(exact, bool) else None}
            return here
        for v in o.values():
            if (found := transport_of(v)) is not None:
                return found
    elif isinstance(o, list):
        for v in o:
            if (found := transport_of(v)) is not None:
                return found
    return None


def metrics(run: Run, memavail: list[tuple[int, int]], held_mib: int = 0) -> Metrics:
    """One run's numbers, from its record and the pod's MemAvailable log when the gate kept one. A capped run's peak VRAM
    is less the holder's `held_mib` (cells.sh: the driver counts the whole GPU)."""
    rec, s = run.record, run.steps
    rows = [r for m in run.measurements for r in (m.get('memory') or {}).get('driver') or []]
    stats = [v.get('stats') or {} for m in run.measurements for v in ((m.get('memory') or {}).get('stats') or {}).get('varena') or []]
    clocks = [c for r in rows if (c := r.get('sm_mhz_mean'))]
    began, ended = rec.get('created_at_ms'), rec.get('settled_at_ms')
    calls = rec.get('calls') or []
    memo = (['run'] if (rec.get('memo') or {}).get('memoized') else []) + [c.get('function') or '?' for c in calls if c.get('memoized')]
    n: dict[str, float | None] = {
        'total_ms': ended - began if began and ended else None, 'execution_ms': run.execution_ms,
        'step1_ms': round(s[0]) if s else None, 'to_step1_ms': run.to_step1_ms,
        'steady_ms': round(sum(s[1:]) / len(s[1:]), 1) if len(s) > 1 else None,  # the mean: SDXL's steps are quantized to ~10 ms
        'denoise_ms': round(sum(s)) if s else None, 'steps': len(s) or None,
        'vram_mib': run.vram['peak'] - held_mib if run.vram else None,
        # The executor's own peak, which ComfyUI's is held against on a whole card: the driver's counts every other
        # executor's resident weights, and varena fills a free card as a cache.
        'proc_mib': max(proc) >> 20 if (proc := [b for m in run.measurements if (b := (m.get('memory') or {}).get('peak_reserved_bytes'))]) else None,
        'host_low_mib': min((kb for at, kb in memavail if began and ended and began / 1000 - 5 <= at <= ended / 1000 + 5), default=None),
        'fills': sum((x.get('weight_residency') or {}).get('fills') or 0 for x in stats),
        'evictions': sum((x.get('weight_residency') or {}).get('evictions') or 0 for x in stats),
        'refilled_gib': round(sum((x.get('working_allocator') or {}).get('weights_filled') or 0 for x in stats) / 2**30, 1),
        'replays': run.replays, 'clock_mhz': round(statistics.median(clocks)) if clocks else None,
        'fork_ms': run.legs.get('fork'), 'cuda_init_ms': run.legs.get('cuda_init'), 'package_import_ms': run.legs.get('package_import'),
        'clock_limit_mhz': next((c for r in rows if (c := r.get('sm_mhz_limit'))), None),
        'throttled': max((r.get('throttled_share') or 0.0 for r in rows), default=None),
        'temperature_c': max((r.get('temperature_max_c') or 0.0 for r in rows), default=None),
        'power_w': max((r.get('power_max_w') or 0.0 for r in rows), default=None),
    }
    if n['host_low_mib'] is not None:
        n['host_low_mib'] = n['host_low_mib'] // 1024
    return {'run': run.number, 'status': run.status, 'reason': run.reason[:200], 'seed': run.seed, 'memo': memo,
            'computed': not memo or (memo[0] != 'run' and len(memo) < len(calls)),
            'numbers': {k: float(v) for k, v in n.items() if v is not None},
            'kernels': {'gemm': run.gemm, 'attention': run.attention, 'graphs': run.graphs},
            'transport': transport_of(rec), 'throttles': sorted({t for r in rows for t in r.get('throttles') or []}), 'output': None,
            'stages': {k: float(v.get('total_ms') or 0.0) for m in run.measurements for k, v in ((m.get('attribution') or {}).get('stages') or {}).items()
                       if not k.startswith('host · ')},
            'series': [float(x) for x in s], 'build': ''}


def output_of(d: str, run: Run, judged: bool) -> Output | None:
    """The run's judged output, found among the gate's project files by digest, and what it is alone."""
    outs = sorted((o for o in run.record.get('output') or [] if (o.get('media_type') or '').split('/')[0] in MEDIA and o.get('sha256')),
                  key=lambda o: MEDIA.index(o.get('media_type', '').split('/')[0]))
    if not outs:
        return None  # a text answer: nothing to look at
    digest, seed = outs[0].get('sha256', '').split(':')[-1], run.seed
    if not judged:
        return {'sha256': digest, 'seed': seed, 'faults': []}
    files = [f for sub in ('project/files', 'proj/files', '../proj/files', '../project/files') for f in glob.glob(os.path.join(d, sub, digest + '.*'))]
    if not files:
        return {'sha256': digest, 'seed': seed, 'faults': ["its output is not among the gate's files"]}
    look = media.alone(files[0])
    return {'sha256': digest, 'seed': seed, 'file': files[0], 'look': look, 'faults': media.faults(look)}


def summary(cold: Metrics | None, warm: list[Metrics]) -> Cell:
    """A cell with no warm runs (a function with no seed: a repeat is the memo's) is judged on its cold run."""
    done = [m for m in (warm or ([cold] if cold else [])) if m['status'] == 'completed' and m['computed']]
    mid: dict[str, float] = {}
    spread: dict[str, float] = {}
    for k in NUMBERS:
        vals = [m['numbers'][k] for m in done if k in m['numbers']]
        if vals:
            mid[k] = statistics.median(vals)
            if k in JUDGED and len(vals) > 1 and mid[k]:
                spread[k] = round((max(vals) - min(vals)) / D2[min(len(vals), 5)] / mid[k], 4)
    runs = ([cold] if cold else []) + warm
    last = done[-1] if done else None
    return {'cold': cold, 'warm': mid, 'spread': spread, 'n': len(done), 'runs': [m['run'] for m in runs],
            'output': cold['output'] if cold and cold['status'] == 'completed' else None,
            'failed': [f"run {m['run']} {m['status']}: {m['reason']}" for m in runs if m['status'] != 'completed']
            + [f"run {m['run']}: the memo answered it ({', '.join(m['memo'])}): it measured nothing" for m in runs if not m['computed']],
            'faults': [f"run {m['run']} output: {w}" for m in runs if m['output'] for w in m['output']['faults']],
            'kernels': last['kernels'] if last else None, 'transport': last['transport'] if last else None,
            'throttles': sorted({t for m in done for t in m['throttles']}),
            'stages': {k: statistics.median(v) for k in {k for m in done for k in m.get('stages', {})}
                       if (v := [m['stages'][k] for m in done if k in m.get('stages', {})])},
            'series': [statistics.median(v) for i in range(max((len(m.get('series', [])) for m in done), default=0))
                       if (v := [m['series'][i] for m in done if len(m.get('series', [])) > i])],
            'builds': sorted({m['build'].split(' on ')[-1] for m in runs if m.get('build')}),
            'senders': sorted({m['build'].split(' on ')[0] for m in runs if ' on ' in m.get('build', '')}), 'cold_downloaded': False}


def text(d: str, name: str) -> str | None:
    path = os.path.join(d, name)
    if not os.path.exists(path):
        return None
    with open(path) as f:
        return f.read().strip()


def reference_of(run: Run | None, facts: str | None = None) -> Reference | None:
    if run is None or run.status != 'completed' or not isinstance(run.result, dict):
        return None
    ref = cast(Reference, dict(run.result))  # the calibrate package's Measured, as the record carries it
    legs = run.legs
    if 'fork' in legs:
        ref['exec_fork_ms'] = legs['fork']
    if 'cuda_init' in legs:
        ref['cuda_init_ms'] = legs['cuda_init']
    rows = [r for m in run.measurements for r in (m.get('memory') or {}).get('driver') or []]
    if clocks := [c for r in rows if (c := r.get('sm_mhz_mean'))]:
        ref['clock_mhz'] = round(statistics.median(clocks))
    if temps := [t for r in rows if (t := r.get('temperature_max_c'))]:
        ref['temperature_c'] = max(temps)
    proc = [b for m in run.measurements if (b := (m.get('memory') or {}).get('peak_reserved_bytes'))]
    if rows and proc and max(r['before_bytes'] for r in rows) < 512 << 20:  # alone on the GPU
        ref['ctx_mib'] = round(max(r['peak_bytes'] for r in rows) / 2**20 - max(proc) / 2**20)
    said = dict(line.split('=', 1) for line in (facts or '').splitlines() if '=' in line)  # pod.sh facts
    if said.get('gpu'):
        ref['gpu'] = said['gpu']
    if said.get('driver'):
        ref['driver'] = said['driver']
    if said.get('pcie'):
        ref['pcie'] = said['pcie']
    if re.fullmatch(r'[0-9.]+', said.get('power_limit_w', '')):
        ref['power_limit_w'] = float(said['power_limit_w'])
    for k in ('disk_stall_s', 'disk_read_gbps'):  # pod.sh disk
        if re.fullmatch(r'[0-9.]+', said.get(k, '')):
            ref[k] = float(said[k])
    return ref


def read_gate(d: str, judged: bool = True) -> Gate:
    """A gate dir's bench cells, pod and reference workload."""
    d = d.rstrip('/')
    memavail: list[tuple[int, int]] = []
    for line in (text(d, 'memavail.log') or '').splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[0].isdigit() and parts[1].isdigit():
            memavail.append((int(parts[0]), int(parts[1])))
    by: dict[str, tuple[list[Metrics], list[Metrics]]] = {}
    for line in (text(d, 'bench.tsv') or '').splitlines():
        cell, role, record, *rest = line.split('\t')
        held = int(rest[0]) if rest and rest[0].isdigit() else 0
        built = ' on '.join(x for x in rest[1:3] if x)  # cells.sh: the sending build, then the machine's cozy
        path = record if record.endswith('.json') else record + '.json'
        path = path if os.path.isabs(path) else os.path.join(d, path)
        run = Run.read(path)
        if run is None:
            m: Metrics = {'run': None, 'status': 'no record', 'reason': record, 'seed': None, 'memo': [], 'computed': True, 'numbers': {},
                          'kernels': {'gemm': [], 'attention': [], 'graphs': None}, 'transport': None, 'throttles': [], 'output': None,
                          'stages': {}, 'series': [], 'build': ''}
        else:
            m = metrics(run, memavail, held)
            if run.status == 'completed':
                m['output'] = output_of(os.path.dirname(path), run, judged)
        m['build'] = built
        cold, warm = by.setdefault(cell, ([], []))
        (cold if role == 'cold' else warm).append(m)
    reference = {w: r for w in ('before', 'after') if (r := reference_of(Run.read(os.path.join(d, f'calibrate-{w}.json')), text(d, 'pod-facts.txt'))) is not None}
    rows = os.path.join(d, 'comfyui-rows.json')
    return {'dir': d, 'group': text(d, 'bench-group') or '?', 'pod': text(d, 'machine'), 'label': text(d, 'label'), 'host': text(d, 'host-ram'), 'reference': reference,
            'invalid': text(d, 'invalid'),
            'cells': {name: summary(cold[0] if cold else None, warm) | {'cold_downloaded': text(d, f'{name}-0-weights.number') == 'none'}
                      for name, (cold, warm) in by.items()},
            'comfyui': cast(dict[str, Comfy], load_json(rows)) if os.path.exists(rows) else {}}  # comfy.py rows wrote it


def cells_file(path: str | None = None) -> list[CellDef]:
    keys = ('name', 'group', 'cap', 'warm', 'seed', 'target', 'args', 'before', 'beats', 'like', 'route', 'comfyui', 'guards')
    rows = []
    with open(path or os.path.join(BENCH, 'cells.tsv')) as f:
        for line in f:
            if line.strip() and not line.startswith('#'):
                vals = (line.rstrip('\n').split('\t') + [''] * len(keys))[:len(keys)]
                rows.append(cast(CellDef, {k: '' if v == '-' else v for k, v in zip(keys, vals)}))  # every key, as strings
    return rows


def newest_baseline() -> str | None:
    files = sorted(glob.glob(os.path.join(HERE, 'baselines', '*.json')), key=lambda p: [int(x) for x in re.findall(r'\d+', os.path.basename(p))])
    return files[-1] if files else None


def sd_of_median(cell: Cell, key: str) -> float:
    return max(cell['spread'].get(key, 0.0), SD_FLOOR) * (1.2533 if cell['n'] > 2 else 1) / math.sqrt(max(cell['n'], 1))


def threshold(base: Cell, cand: Cell, key: str, pod_sd: float) -> float:
    """How far over its baseline a judged time may be: the bar, or three standard deviations of the ratio of the two
    medians (each cell's own measured spread, and what stays between two pods), whichever is wider."""
    return max(BAR, 3 * math.sqrt(sd_of_median(base, key) ** 2 + sd_of_median(cand, key) ** 2 + pod_sd ** 2))


def ref_file(refs: str, asset: str) -> str | None:
    """A stored reference output, fetched from the baselines release when `refs` lacks it."""
    path = os.path.join(refs, asset)
    if not os.path.exists(path):
        os.makedirs(refs, exist_ok=True)
        subprocess.run(['gh', 'release', 'download', TAG, '-R', REPO, '-D', refs, '-p', asset], capture_output=True)
    return path if os.path.exists(path) else None


def judge_output(d: CellDef, c: Cell, b: Cell | None, others: list[Gate], refs: str, like_group: str | None) -> tuple[list[str], list[str]]:
    """Why a cell's judged output fails, and notes: against its stored reference, and against `like`'s of the same request
    and seed when `like`'s group ran beside this one."""
    why: list[str] = []
    o = c['output']
    if not o or 'file' not in o or 'look' not in o:
        return why, []
    stored = b['output'] if b else None
    if stored and stored['seed'] != o['seed']:
        return why, [f"its stored output is seed {stored['seed']}, this one {o['seed']}: not compared"]
    if stored and 'look' in stored and 'asset' in stored:
        why += [w for w in media.faults(o['look'], stored['look']) if w not in o['faults']]
        ref = ref_file(refs, stored['asset'])
        why += media.differs(media.pair(ref, o['file'])) if ref else [f"reference output {stored['asset']} is not in {refs} or the {TAG} release"]
    if d['like'] and like_group and any(in_group(like_group, g['group']) for g in others):
        found = [x for g in others if in_group(like_group, g['group']) and (y := g['cells'].get(d['like'])) and (x := y['output']) and 'file' in x]
        if found and found[0]['seed'] != o['seed']:
            why.append(f"{d['like']}'s output is seed {found[0]['seed']}, this one {o['seed']}: cells.tsv must give the two one seed")
        elif found and 'file' in found[0]:
            why += media.differs(media.pair(found[0]['file'], o['file']), f"{d['like']}'s output (same request and seed)")
        else:
            why.append(f"no {d['like']} output of this candidate to hold it against")
    return why, []


def judge(defs: list[CellDef], cand: Gate, base: Group | None, version: str | None, known: list[Known], others: list[Gate],
          refs: str | None, groups: dict[str, str]) -> list[Verdict]:
    """Each cell of the group's list (`defs`) against the group's stored baseline; `groups`: every cell's group."""
    out = []
    bcells, comfy = (base['cells'], base['comfyui']) if base else ({}, {})
    same_pod = bool(base and base['pod'] and base['pod'] == cand['pod'])
    for d in defs:
        c, b = cand['cells'].get(d['name']), bcells.get(d['name'])
        v = Verdict(d['name'], b, c, cand['comfyui'].get(d['name']) or comfy.get(d['name']))  # this pod's ComfyUI, else the stored
        out.append(v)
        if not c or (not c['n'] and not c['failed']):
            v.verdict, v.why = 'FAIL', ['not run']
            continue
        v.why += c['failed'] + c['faults']
        if refs is not None:
            why, notes = judge_output(d, c, b, others, refs, groups.get(d['like']))
            v.why += why
            v.notes += notes
            if d['like'] and not any(in_group(groups.get(d['like'], ''), g['group']) for g in others):
                v.notes.append(f"its output is held against {d['like']}'s by the table of every group")
        w = c['warm']
        if d['cap'] and w.get('vram_mib', 0) > float(d['cap']) * 1024:
            v.why.append(f"peak VRAM {w['vram_mib'] / 1024:.2f} GiB over its {d['cap']} GiB limit")
        t = c['transport'] or {'route': None, 'degree': None, 'mapped': None, 'exact': None}
        if d['route'] and c['n'] and (t['route'] != d['route'] or (d['route'] != 'nccl' and not (t['mapped'] and t['exact']))):
            v.why.append(f"transport {t['route']} mapped={t['mapped']} exact={t['exact']}, not {d['route']}")
        if d['beats'] and c['n']:
            one = cand['cells'].get(d['beats'])
            if not one or not one['n']:
                v.why.append(f"no {d['beats']} run to beat")
            else:
                for k in ('total_ms', 'denoise_ms'):
                    if k in w and k in one['warm'] and w[k] >= one['warm'][k]:
                        v.why.append(f"{k[:-3]} {w[k] / 1000:.1f} s does not beat {d['beats']}'s {one['warm'][k] / 1000:.1f} s on one GPU")
        clock = 1.0
        if b and b['n'] and c['n']:
            bw = b['warm']
            scaled = not d['cap'] and 'clock_mhz' in w and 'clock_mhz' in bw and not same_pod
            clock = w['clock_mhz'] / bw['clock_mhz'] if scaled else 1.0
            pod_sd = 0.0 if same_pod else POD_SD if scaled else POD_SD_UNSCALED
            if not scaled and not same_pod:
                v.notes.append('capped: judged unscaled' if d['cap'] else 'no GPU clock on one side: judged unscaled')
            for k in JUDGED:
                if k not in w or k not in bw:
                    if k in bw:
                        v.notes.append(f'no {k[:-3]} number now (baseline {bw[k] / 1000:.2f} s)')
                    continue
                # The SM clock paces GPU work only: a step wholly, a total by its denoise.
                gpu = w[k] if k == 'steady_ms' else min(w.get('denoise_ms', 0.0), w[k])
                raw, cal = w[k] / bw[k], (w[k] - gpu + gpu * clock) / bw[k]
                limit = threshold(b, c, k, pod_sd)
                v.ratios[k] = (raw, cal, limit)
                if cal > 1 + limit and (k != 'total_ms' or w[k] - bw[k] > TOTAL_FLOOR_MS):
                    v.why.append(f"{k[:-3]} {w[k] / 1000:.2f} s, {cal:.2f}x its baseline's {bw[k] / 1000:.2f} s (raw {raw:.2f}x; at most {1 + limit:.2f}x)")
            bc, cc = b['cold']['numbers'] if b['cold'] else {}, c['cold']['numbers'] if c['cold'] else {}
            if 'total_ms' in bc and 'total_ms' in cc and cc['total_ms'] > (1 + COLD_NOTE) * bc['total_ms'] + 1000:
                v.notes.append(f"cold {cc['total_ms'] / 1000:.1f} s against {bc['total_ms'] / 1000:.1f} s")
            slow = [f"{k[:-3].replace('_', ' ')} {w[k] / 1000:.2f} s against {bw[k] / 1000:.2f} s" for k in ('package_import_ms', 'cuda_init_ms')
                    if k in w and k in bw and bw[k] >= 250 and w[k] > SLOW_LEG * bw[k]]
            if slow:
                v.notes.append(f"slower host CPU: {', '.join(slow)}")
            if bw.get('fills') and w.get('refilled_gib', 0) > 1.5 * bw.get('refilled_gib', 0) + 1:
                v.notes.append(f"refilled {w['refilled_gib']:.0f} GiB a call, baseline {bw.get('refilled_gib', 0):.0f}")
            if b['kernels'] and c['kernels'] and (b['kernels']['gemm'], b['kernels']['attention']) != (c['kernels']['gemm'], c['kernels']['attention']):
                v.notes.append(f"kernels {b['kernels']['gemm']} / {b['kernels']['attention']} -> {c['kernels']['gemm']} / {c['kernels']['attention']}")
            if b['kernels'] and c['kernels'] and b['kernels']['graphs'] != c['kernels']['graphs']:
                v.notes.append(f"CUDA graphs {b['kernels']['graphs']} -> {c['kernels']['graphs']} (replays {bw.get('replays', 0):.0f} -> {w.get('replays', 0):.0f})")
        elif c['n']:
            v.notes.append('no baseline: this run starts it')
        x, was = v.comfy, comfy.get(d['name'])
        if x and x['total_s'] is None:
            v.notes.append(f"ComfyUI: no warm number ({x['status']})")
        elif x and x['total_s'] and b and b['n'] and c['n'] and 'total_ms' in w:
            ours = w['total_ms'] / 1000 if x is not was else (w['total_ms'] - w.get('denoise_ms', 0.0) * (1 - clock)) / 1000
            won = was is not None and was['total_s'] is not None and b['warm']['total_ms'] / 1000 < was['total_s']
            if won and x['total_s'] <= ours:
                v.why.append(f"loses to ComfyUI ({ours:.1f} s against {x['total_s']:.1f} s) where {version} won")
        if len(c.get('builds', [])) > 1:
            v.notes.append(f"ran on more than one machine build: {', '.join(c['builds'])}")
        if len(c.get('senders', [])) > 1 or any('>' in b for b in c.get('senders', [])):
            v.notes.append(f"sent by {len(c['senders'])} builds of this computer's cozy (the machine's stayed)")
        if 'sw_thermal' in c['throttles']:
            v.notes.append(f"thermal throttling ({w.get('temperature_c')} C, throttled {w.get('throttled', 0):.0%} of the time)")
        issue = next((k for k in known if k['cell'] == d['name'] and k.get('group', cand['group']) == cand['group']), None)
        v.verdict = 'PASS' if not v.why else 'KNOWN' if issue else 'FAIL'
        if issue:
            v.notes.append(f"known issue {issue['issue']}: {issue['reason']}" + ('' if v.why else ' (passes now: remove it)'))
    return out


def pod_check(cand: Gate, stored: Reference | None) -> tuple[list[str], list[str]]:
    """Why the pod is not a pod of its GPU type's class (its reference workload against the stored one, its host CPU against
    the ceilings), and notes: the workload after the cells against before."""
    ref, out, notes = cand['reference'], [], []
    before = ref.get('before')
    if not before:
        return ['no reference workload on this pod'], []
    gpus = int(m.group(1)) if (m := re.search(r'x(\d+)(\.|$)', cand['group'])) else 1  # <sku>x<gpus>[.<part>]
    if (stall := before.get('disk_stall_s')) is not None and stall >= DISK_STALL_S:
        out.append(f"disk stalled {stall:.1f} s (requests in flight, none completed)")
    for k, most in HOST_CEILING.items():
        most += CUDA_INIT_PER_GPU * (gpus - 1) if k == 'cuda_init_ms' else 0
        now = cast(float | None, before.get(k))
        if now and now > most:
            out.append(f"{k} {now:.0f} ms: a slow host CPU (a normal 4090 host is under {most:.0f})")
    for k, by in POD_RANGE.items():
        now, was = cast(float | None, before.get(k)), cast(float | None, (stored or {}).get(k))
        if now and was and (1 - now / was > by if k.endswith('_gbps') else now / was - 1 > by):
            out.append(f"{k} {now:.3g} against this GPU type's {was:.3g} ({now / was - 1:+.0%}, at most {by:.0%} worse)")
    after = ref.get('after') or {}
    for k in ('attention_ms', 'conv_ms', 'gemm_ms'):
        a, b = cast(float | None, after.get(k)), cast(float | None, before.get(k))
        if a and b and abs(a / b - 1) > DRIFT:
            notes.append(f"{k} drifted {a / b - 1:+.1%} over the run ({before.get('temperature_c')} -> {after.get('temperature_c')} C)")
    return out, notes


def fmt(cell: Cell | None) -> str:
    if not cell or not cell['n']:
        return 'failed' if cell and cell['failed'] else '-'
    w = cell['warm']

    def f(k: str, div: float = 1000.0, digits: int = 2) -> str:
        return '-' if k not in w else f'{w[k] / div:.{digits}f}'
    cold = cell['cold']['numbers'].get('total_ms') if cell['cold'] else None
    return f"{f('steady_ms')} / {f('total_ms', digits=1)} / {'-' if cold is None else f'{cold / 1000:.1f}'} / {f('vram_mib', 1024, 1)}"


def table(gates: list[Gate], base: Baseline | None, defs: list[CellDef], known: list[Known], refs: str | None,
          beside: list[Gate] | None = None) -> tuple[list[str], list[str]]:
    """The gates' groups judged, as markdown lines, and the failing cells. `beside`: more groups, only for `like`."""
    fails, lines = [], []
    version = base['version'] if base else None
    groups = {d['name']: d['group'] for d in defs}
    for g in gates:
        if g['invalid']:
            lines += [f"\n### {g['group']} on {g['pod']}: INVALID, not judged: {g['invalid']}"]
            continue
        group = base['groups'].get(g['group']) if base else None
        verdicts = judge([d for d in defs if in_group(d['group'], g['group'])], g, group, version, known, gates + (beside or []), refs, groups)
        ref = g['reference'].get('before') or {}
        builds = sorted({b for c in g['cells'].values() for b in c.get('builds', [])})
        lines += [f"\n### {g['group']} on {g['pod']}: {g['label']} against {version if group else 'no baseline'}"
                  f"; sent by {', '.join(builds) or '?'}"
                  f"{' (' + str(group['pod']) + ')' if group else ''}",
                  f"pod: {ref.get('gpu', '?')}, driver {ref.get('driver', '?')}, PCIe {ref.get('pcie', '?')}, CPU {ref.get('cpu', '?')} x{ref.get('cores', '?')}"
                  f" (python {ref.get('python_ms', '?')} ms, launch {ref.get('launch_us', '?')} us, fork {ref.get('exec_fork_ms', '?')} ms,"
                  f" cuda_init {ref.get('cuda_init_ms', '?')} ms), disk stall {ref.get('disk_stall_s', '?')} s at {ref.get('disk_read_gbps', '?')} GB/s,"
                  f" h2d {ref.get('h2d_gbps', '?')} GB/s, host {ref.get('host_gib', '?')} GiB"
                  f" (limit {ref.get('limit_gib', '-')}), {ref.get('temperature_c', '?')} C at start",
                  f"host RAM: {g['host'] or '?'}", '',
                  '| cell | baseline: steady s / warm total s / cold total s / peak GiB | candidate | steady x | total x | ComfyUI s | verdict |',
                  '|---|---|---|---|---|---|---|']
        for v in verdicts:
            def r(k: str) -> str:
                return '-' if k not in v.ratios else f'{v.ratios[k][1]:.2f} (raw {v.ratios[k][0]:.2f}, limit {1 + v.ratios[k][2]:.2f})'
            lines.append(f"| {v.name} | {fmt(v.base)} | {fmt(v.cand)} | {r('steady_ms')} | {r('total_ms')} | {v.comfy['total_s'] if v.comfy else '-'} |"
                         f" {v.verdict}{': ' + '; '.join(v.why) if v.why else ''}{' (' + '; '.join(v.notes) + ')' if v.notes else ''} |")
            if v.verdict == 'FAIL':
                fails.append(f"{g['group']} {v.name}: {'; '.join(v.why)}")
        bad, notes = pod_check(g, group['reference'] if group else None)
        for p in bad:  # its numbers are another class of pod's: no verdict on the code from them
            lines.append(f"\npod {g['pod']} OUT OF RANGE: {p}")
            fails.append(f"{g['group']} pod {g['pod']}: {p}")
        lines += [f"\npod {g['pod']}: {p}" for p in notes]
    return lines, fails


def versus(gates: list[Gate], defs: list[CellDef]) -> tuple[list[str], list[str]]:
    """Each cell with a ComfyUI row on its pod, both engines side by side, as markdown lines, and every cell where Cozy is
    slower (warm total, steady step) or holds more GPU memory, with the gap. Peak VRAM on a whole card: Cozy's executor's
    own (with its CUDA context, measured on this pod) against ComfyUI's NVML peak, ComfyUI alone on the GPU; on a capped
    cell both are the driver's less the holder's. Cold is shown, not judged: Cozy's is its first call after the weights
    landed, ComfyUI's a new server with the page cache dropped, its start not counted."""
    cap = {d['name']: d['cap'] for d in defs}
    lines: list[str] = []
    losses: list[str] = []

    def sec(v: float | None) -> float | None:
        return None if v is None else v / 1000

    def gib(v: float | None) -> float | None:
        return None if v is None else v / 1024

    def ours(g: Gate, name: str) -> tuple[dict[str, float], float | None, float | None]:
        """A cell's warm medians, its peak VRAM as held against ComfyUI's, and its cold total."""
        c = g['cells'].get(name)
        w = c['warm'] if c and c['n'] else {}
        ctx = (g['reference'].get('before') or {}).get('ctx_mib')
        proc = w.get('proc_mib')
        peak = w.get('vram_mib') if cap.get(name) else (proc + (ctx or 0) if proc is not None else None)
        cold = c['cold']['numbers'].get('total_ms') if c and c['cold'] and c['cold']['status'] == 'completed' else None
        return w, peak, cold

    for g in gates:
        if not g['comfyui'] or g['invalid']:
            continue
        ctx = (g['reference'].get('before') or {}).get('ctx_mib')
        lines += [f"\n### {g['group']} on {g['pod']}: {g['label']} against ComfyUI {next(iter(g['comfyui'].values()))['comfyui']}"
                  f" (Cozy's CUDA context {ctx if ctx is not None else '?'} MiB; a cold marked * fetched its weights: no --warm on that build)", '',
                  '| cell | GPU | warm total s: Cozy / ComfyUI | ComfyUI / Cozy | steady step s: Cozy / ComfyUI | peak VRAM GiB: Cozy / ComfyUI |'
                  ' cold s: Cozy / ComfyUI | Cozy loses |', '|---|---|---|---|---|---|---|---|']
        for name, x in g['comfyui'].items():
            c = g['cells'].get(name)
            w, ours_peak, cold = ours(g, name)
            judged = {'warm total': (sec(w.get('total_ms')), x['total_s'], 's'), 'steady step': (sec(w.get('steady_ms')), x.get('steady_s'), 's'),
                      'peak VRAM': (gib(ours_peak), gib(x.get('peak_mib')), 'GiB')}
            gaps = [f"{k} +{a - b:.3g} {u} ({a / b - 1:+.0%})" for k, (a, b, u) in judged.items() if a is not None and b and a > b]
            mixed = f" (ran on {len(c['builds'])} machine builds: rerun it)" if c and len(c.get('builds', [])) > 1 else ''
            if not w:
                gaps = [f"Cozy did not complete: {'; '.join(c['failed'][:1]) if c else 'not run'}"]
            elif x['status'] != 'ok' and x['total_s'] is None:
                gaps = []  # ComfyUI did not complete: a completion win
            if gaps:
                losses.append(f"{g['group']} {name}: {'; '.join(gaps)}")

            def pair(a: float | None, b: float | None, digits: int) -> str:
                def f(v: float | None) -> str:
                    return '-' if v is None else f'{v:.{digits}f}'
                return f'{f(a)} / {f(b)}'
            tot, steady, peak = judged['warm total'], judged['steady step'], judged['peak VRAM']
            speed = f"{tot[1] / tot[0]:.2f}x" if tot[0] and tot[1] else ('ComfyUI failed' if x['status'] != 'ok' else '-')
            dl = '*' if c and c.get('cold_downloaded') else ''
            lines.append(f"| {name} | {f'{cap[name]} GiB' if cap.get(name) else 'whole'} | {pair(tot[0], tot[1], 2)} | {speed} |"
                         f" {pair(steady[0], steady[1], 3)} | {pair(peak[0], peak[1], 1)} | {pair(sec(cold), x['cold_s'], 1)}{dl} |"
                         f" {'; '.join(gaps) or ('no: ComfyUI ' + x['status'] if x['status'] != 'ok' else 'no')}{mixed} |")
        for name, x in g['comfyui'].items():
            if next((d['comfyui'] for d in defs if d['name'] == name), '').startswith('h3'):
                lines += stage_lines(name, g, x)
    return lines, losses


# H3's stages in both engines: Cozy's attribution stage names, ComfyUI's node classes (substrings).
H3_STAGES = (('text encode', ('condition_text',), ('TextEncode', 'ImageToVideo')), ('video decode', ('decode_video',), ('VAEDecode',)),
             ('audio decode', ('decode_audio',), ('VAEDecodeAudio',)))


def stage_lines(name: str, g: Gate, x: Comfy) -> list[str]:
    """A video cell's time by stage, both engines, cold and warm: text encode, the dense and the sparse steps (H3 Turbo's
    eight: four dense, then four sparse), video and audio decode, and the total."""
    c = g['cells'].get(name)
    tiers = x.get('tiers') or {}
    if not c or not tiers:
        return []
    warm = [t for k, t in tiers.items() if k != 'cold' and t['e2e_s'] is not None]

    def comfy(t: list[Tier], f: Callable[[Tier], float | None]) -> float | None:
        vals = [v for one in t if (v := f(one)) is not None]
        return statistics.median(vals) if vals else None

    def node(t: Tier, keys: tuple[str, ...]) -> float | None:
        found = [v for k, v in t['stages'].items() if any(n in k for n in keys) and not (keys == ('VAEDecode',) and 'Audio' in k)]
        return sum(found) if found else None

    def half(series: list[float], which: int) -> float | None:
        part = series[: len(series) // 2] if which == 0 else series[len(series) // 2:]
        return sum(part) / len(part) if part else None

    def cozy(st: dict[str, float], series: list[float], total: float | None) -> list[float | None]:
        out: list[float | None] = []
        for _, ours, _ in H3_STAGES[:1]:
            out.append(sum(st[k] for k in ours if k in st) / 1000 if any(k in st for k in ours) else None)
        out += [None if (h := half(series, 0)) is None else h / 1000, None if (h := half(series, 1)) is None else h / 1000]
        for _, ours, _ in H3_STAGES[1:]:
            out.append(sum(st[k] for k in ours if k in st) / 1000 if any(k in st for k in ours) else None)
        return out + [None if total is None else total / 1000]

    def theirs(t: list[Tier]) -> list[float | None]:
        out: list[float | None] = [comfy(t, lambda one: node(one, H3_STAGES[0][2]))]
        out += [comfy(t, lambda one: half(one['steps_s'], 0)), comfy(t, lambda one: half(one['steps_s'], 1))]
        out += [comfy(t, partial(node, keys=keys)) for _, _, keys in H3_STAGES[1:]]
        return out + [comfy(t, lambda one: one['e2e_s'])]
    cold = c['cold']
    cols = [cozy(cold['stages'], cold['series'], cold['numbers'].get('total_ms')) if cold else [None] * 6,
            theirs([tiers['cold']]) if 'cold' in tiers else [None] * 6,
            cozy(c['stages'], c['series'], c['warm'].get('total_ms')), theirs(warm)]
    rows = ['text encode', 'steps 1-4 (dense), each', 'steps 5-8 (sparse), each', 'video decode', 'audio decode', 'total']
    out = [f"\n#### {name} by stage, s ({g['pod']})", '', '| stage | Cozy cold | ComfyUI cold | Cozy warm | ComfyUI warm |', '|---' * 5 + '|']
    for i, r in enumerate(rows):
        out.append(f"| {r} | " + ' | '.join('-' if col[i] is None else f'{col[i]:.1f}' for col in cols) + ' |')
    return out


MODEL_NAMES = {'h3_turbo': 'H3 Turbo 15 s', 'h3_full': 'H3 30-step 15 s', 'sdxl': 'SDXL 1024²', 'sdxl_4k': 'SDXL 4096² HiDiffusion',
               'anima': 'Anima 1536²', 'moss': 'MOSS-Image 4 MP', 'qwen': 'Qwen-Image 2.1 4 MP', 'krea2': 'Krea 2 Turbo 4 MP',
               'flux2': 'FLUX.2 [dev] 1024²', 'qwen_image': 'Qwen-Image-2512', 'ideogram4': 'Ideogram 4'}
# Diverging, blue (Cozy faster) to red (slower) about a gray "within noise" (dataviz reference palette: blue ramp, red
# from its categorical step, the gray midpoint); the printed ratio carries the sign, so color never stands alone. Light
# hexes are the marks' own attributes (any renderer); dark mode overrides them by class.
FILLS = {'w1': ('#cde2fb', '#184f95'), 'w2': ('#9ec5f4', '#1c5cab'), 'w3': ('#6da7ec', '#256abf'), 'mid': ('#f0efec', '#383835'),
         'l1': ('#f9d3d2', '#6b2a29'), 'l2': ('#f2a6a5', '#8f3534'), 'l3': ('#e34948', '#b8403f'), 'none': ('#fcfcfb', '#1a1a19')}
INK = {'bg': ('#fcfcfb', '#1a1a19'), 'ink': ('#0b0b0b', '#f0efec'), 'muted': ('#52514e', '#c3c2b7')}
_DARK = ' '.join(f'.{k} {{ fill: {d}; }}' for k, (_, d) in {**FILLS, **INK}.items())
CHART_CSS = f"""
text {{ font: 13px system-ui, sans-serif; }} .h {{ font-weight: 600; }} .sm {{ font-size: 12px; }} rect.cell {{ stroke-width: 2; }}
@media (prefers-color-scheme: dark) {{ :root:where(:not([data-theme="light"])) {{ {_DARK} rect.cell {{ stroke: #1a1a19; }} }} }}
:root[data-theme="dark"] {{ {_DARK} rect.cell {{ stroke: #1a1a19; }} }}
"""


def chart(gates: list[Gate], defs: list[CellDef], title: str) -> str:
    """The ComfyUI comparison as an SVG grid: a row per model and GPU memory, a column per GPU, each cell ComfyUI's warm
    total over Cozy's (above 1: Cozy faster), filled by it; a failed ComfyUI is Cozy's (a completion win)."""
    import html
    key = {d['name']: (d['comfyui'], d['cap']) for d in defs}
    grid: dict[str, dict[str, tuple[str, str, str]]] = {}  # row -> GPU -> (text, fill, tooltip)
    gpus: list[str] = []
    for g in gates:
        if g['invalid']:
            continue
        base = g['group'].split('.')[0]
        sku, n = base.rsplit('x', 1)
        gpu = sku.upper().replace('RTX-', 'RTX ').replace('-', ' ') + (f' x{n}' if n != '1' else '')
        for name, x in g['comfyui'].items():
            k, cap = key.get(name, ('', ''))
            if not k or (g['group'].endswith('.matrix2') and name.endswith('-g2')):
                continue
            row = MODEL_NAMES.get(k, k) + (f' at {cap} GiB' if cap else '')
            c = g['cells'].get(name)
            ours = c['warm'].get('total_ms', 0.0) / 1000 if c and c['n'] else None
            theirs = x['total_s']
            if ours is None:
                cell = ('Cozy failed', 'l3', f'{name}: Cozy did not complete')
            elif theirs is None:
                cell = ('ComfyUI failed', 'w3', f"{name}: ComfyUI {x['status'][:80]}; Cozy {ours:.1f} s")
            else:
                r = theirs / ours
                fill = ('w3' if r >= 1.6 else 'w2' if r >= 1.25 else 'w1' if r >= 1.03
                        else 'mid' if r > 0.97 else 'l1' if r > 0.8 else 'l2' if r > 0.625 else 'l3')
                cell = (f'{r:.2f}x', fill, f'{name} on {gpu}: Cozy {ours:.1f} s, ComfyUI {theirs:.1f} s warm')
            grid.setdefault(row, {})[gpu] = cell
            if gpu not in gpus:
                gpus.append(gpu)
    rows = [r for r in [MODEL_NAMES[k] + suffix for k in MODEL_NAMES for suffix in ('', ' at 8 GiB', ' at 6 GiB')] if r in grid]
    rows += [r for r in grid if r not in rows]
    lw, cw, rh, top = 230, 112, 30, 64
    width, height = max(lw + cw * len(gpus) + 16, 760), top + rh * len(rows) + 48
    ink, muted = INK['ink'][0], INK['muted'][0]
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img"'
           f' aria-label="{html.escape(title)}"><style>{CHART_CSS}</style><rect class="bg" width="100%" height="100%" fill="{INK["bg"][0]}"/>',
           f'<text class="h ink" x="8" y="22" fill="{ink}">{html.escape(title)}</text>',
           f'<text class="sm muted" x="8" y="42" fill="{muted}">ComfyUI warm time / Cozy warm time, same pod: above 1x Cozy is faster; gray is within 3%</text>']
    for j, gpu in enumerate(gpus):
        out.append(f'<text class="h ink" x="{lw + cw * j + cw / 2}" y="{top - 6}" text-anchor="middle" fill="{ink}">{html.escape(gpu)}</text>')
    for i, row in enumerate(rows):
        y = top + rh * i
        out.append(f'<text class="ink" x="8" y="{y + rh / 2 + 5}" fill="{ink}">{html.escape(row)}</text>')
        for j, gpu in enumerate(gpus):
            t, k, tip = grid[row].get(gpu, ('', 'none', 'not run'))
            x0 = lw + cw * j
            out.append(f'<g><title>{html.escape(tip)}</title><rect class="cell {k}" x="{x0}" y="{y}" width="{cw}" height="{rh}" rx="4"'
                       f' fill="{FILLS[k][0]}" stroke="{INK["bg"][0]}"/><text class="ink" x="{x0 + cw / 2}" y="{y + rh / 2 + 5}"'
                       f' text-anchor="middle" fill="{ink}">{html.escape(t or "-")}</text></g>')
    out.append(f'<text class="sm muted" x="8" y="{height - 18}" fill="{muted}">Raw rows: each pod\'s gate dir (bench.tsv, comfyui-rows.json);'
               ' bench.py versus for steady step, peak VRAM and cold.</text>')
    return '\n'.join(out + ['</svg>'])


def scaling(gates: list[Gate], defs: list[CellDef]) -> list[str]:
    """Each multi-GPU cell against the one-GPU cell it must beat (`beats`), on the same pod: the transport its group used
    (route, degree, mapped/exact), warm total and denoise, the steps by half (H3 Turbo: four dense, then four sparse), and
    the speed-up."""
    out = ['| pod | cells | transport | warm total s: 1 GPU / N | speed-up | denoise s: 1 / N | dense step s: 1 / N | sparse step s: 1 / N |',
           '|---|---|---|---|---|---|---|---|']

    def half(series: list[float], which: int) -> float | None:
        part = series[: len(series) // 2] if which == 0 else series[len(series) // 2:]
        return sum(part) / len(part) / 1000 if part else None

    def f(v: float | None, d: int = 1) -> str:
        return '-' if v is None else f'{v:.{d}f}'
    for g in gates:
        for d in defs:
            if g['invalid'] or not d['beats'] or not in_group(d['group'], g['group']):
                continue
            many, one = g['cells'].get(d['name']), g['cells'].get(d['beats'])
            if not many or not one:
                continue
            t = many['transport']
            route = f"{t['route']} x{t['degree']} mapped={t['mapped']} exact={t['exact']}" if t else 'none recorded'
            a, b = one['warm'].get('total_ms'), many['warm'].get('total_ms')
            da, db = one['warm'].get('denoise_ms'), many['warm'].get('denoise_ms')
            out.append(f"| {g['group']} on {g['pod']} | {d['beats']} / {d['name']} | {route} | {f(a and a / 1000)} / {f(b and b / 1000)} |"
                       f" {f(a / b if a and b else None, 2)}x | {f(da and da / 1000)} / {f(db and db / 1000)} |"
                       f" {f(half(one['series'], 0), 2)} / {f(half(many['series'], 0), 2)} | {f(half(one['series'], 1), 2)} / {f(half(many['series'], 1), 2)} |")
    return out


def baseline(version: str, gates: list[Gate], refs: str) -> Baseline:
    """The gates as a release's stored numbers, Cozy's only; each cell's judged output copied into `refs` under its asset name."""
    os.makedirs(refs, exist_ok=True)
    out: Baseline = {'version': version, 'groups': {}}
    for g in gates:
        for name, c in g['cells'].items():
            o = c['output']
            if o and 'file' in o:
                asset = f"bench-{version}-{g['group']}-{name}{os.path.splitext(o['file'])[1]}"
                shutil.copyfile(o['file'], os.path.join(refs, asset))
                stored: Output = {'sha256': o['sha256'], 'seed': o['seed'], 'faults': o['faults'], 'asset': asset}
                if 'look' in o:
                    stored['look'] = o['look']
                c['output'] = stored
            if c['cold'] and c['cold']['output']:
                c['cold']['output'] = None  # the cell's own output is the stored one
        out['groups'][g['group']] = {'pod': g['pod'], 'label': g['label'], 'host': g['host'], 'reference': g['reference'].get('before') or {},
                                     'cells': g['cells'], 'comfyui': {}}
    return out


def known_issues(path: str | None = None) -> list[Known]:
    path = path or os.path.join(BENCH, 'known-issues.json')
    return cast(list[Known], load_json(path)) if os.path.exists(path) else []  # rows of Known


def check(gate: dict[str, object]) -> tuple[list[str], list[str]]:
    """analyze.py's check for suite bench: this gate's group against the newest stored baseline, beside the groups bench.sh
    ran into the same directory. The table goes to bench-table.md."""
    out = os.path.abspath(str(gate['out']))
    here = read_gate(out)
    beside = [read_gate(d) for d in sorted(glob.glob(os.path.join(os.path.dirname(out), '*')))
              if d != out and os.path.exists(os.path.join(d, 'bench-group')) and os.path.exists(os.path.join(d, 'bench.tsv'))]
    path = newest_baseline()
    base = cast(Baseline, load_json(path)) if path else None  # bench.py baseline's own file
    lines, fails = table([here], base, cells_file(), known_issues(), os.path.join(HERE, 'baselines', 'refs'), beside)
    with open(os.path.join(out, 'bench-table.md'), 'w') as f:
        f.write('\n'.join(lines) + '\n')
    return [f'bench {f}' for f in fails], [f"bench: {here['group']} against {base['version'] if base else 'no baseline'}, table in bench-table.md"]


def main(argv: list[str]) -> int:
    flags = ('--baseline', '--cells', '--known', '--comfyui', '--comfyui-out', '--refs', '--out', '--title')
    opt = {f: argv[argv.index(f) + 1] for f in flags if f in argv}
    skip = {i for f in opt for i in (argv.index(f), argv.index(f) + 1)}
    args = [a for i, a in enumerate(argv) if i not in skip and not a.startswith('--')]
    if not args:
        print(__doc__, file=sys.stderr)
        return 2
    verb, rest = args[0], args[1:]
    defs = cells_file(opt.get('--cells'))
    known = known_issues(opt.get('--known'))
    base_file = opt.get('--baseline') or newest_baseline()
    base = cast(Baseline, load_json(base_file)) if base_file and os.path.exists(base_file) else None  # bench.py baseline's own file
    if base and '--comfyui' in opt:  # the release's ComfyUI rows, kept off the repo (baseline --comfyui-out)
        for name, rows in cast(dict[str, dict[str, Comfy]], load_json(opt['--comfyui'])).items():
            if name in base['groups']:
                base['groups'][name]['comfyui'] = rows
    refs = opt.get('--refs') or os.path.join(HERE, 'baselines', 'refs')
    if verb == 'table':
        judged = '--no-output' not in argv
        lines, fails = table([read_gate(d, judged) for d in rest], base, defs, known, refs if judged else None)
        print('\n'.join(lines))
        if not judged:
            print('\noutputs NOT judged (--no-output): no verdict for a release')
        print('\n' + '\n'.join(f'FAIL {f}' for f in fails) if fails else '')
        print(f"VERDICT {'FAIL' if fails else 'PASS'} ({len(fails)} failing cells)")
        return 1 if fails else 0
    if verb == 'scaling':
        print('\n'.join(scaling([read_gate(d, judged=False) for d in rest], defs)))
        return 0
    if verb == 'chart':
        svg = chart([read_gate(d, judged=False) for d in rest], defs, opt.get('--title', 'Cozy against ComfyUI'))
        with open(opt.get('--out', 'chart.svg'), 'w') as f:
            f.write(svg)
        return 0
    if verb == 'versus':
        lines, losses = versus([read_gate(d, judged=False) for d in rest], defs)
        print('\n'.join(lines))
        print('\n' + '\n'.join(f'LOSS {x}' for x in losses) if losses else '\nno losses')
        return 0
    if verb == 'baseline':
        gates = [read_gate(d) for d in rest[1:]]
        if '--comfyui-out' in opt:
            with open(opt['--comfyui-out'], 'w') as f:
                json.dump({g['group']: g['comfyui'] for g in gates if g['comfyui']}, f, indent=1, sort_keys=True)
        written = baseline(rest[0], gates, refs)
        path = opt.get('--out') or os.path.join(HERE, 'baselines', rest[0] + '.json')
        os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
        with open(path, 'w') as f:
            json.dump(written, f, indent=1, sort_keys=True)
        print(path)
        return 0
    if verb == 'admit':
        g = read_gate(rest[0], judged=False)
        group = base['groups'].get(g['group']) if base else None
        bad, _ = pod_check(g, group['reference'] if group else None)
        print('\n'.join(bad) or f"pod {g['pod']} in range" + ('' if group else ' (no stored numbers for its GPU type: the ceilings only)'))
        return 1 if bad else 0
    if verb == 'noise':
        for g in (read_gate(d, judged=False) for d in rest):
            for name, c in g['cells'].items():
                print(f"{g['group']}\t{g['pod']}\t{name}\tn={c['n']}\t" + '\t'.join(f"{k[:-3]} ±{c['spread'].get(k, 0):.1%}" for k in JUDGED))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
