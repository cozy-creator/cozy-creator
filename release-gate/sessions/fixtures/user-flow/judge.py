"""user-flow's verdict (xs-203): what e2e/cli/web/user-flow.spec.js saw and kept in <flow>, judged, as Markdown tables on stdout
(the session puts them in report.md and the job summary) and one `PASS|FAIL|NOTE <check>: <detail>` line each in <flow>/verdict.txt.
Exit 1 on any FAIL.
    judge.py <flow dir> <bench.json> <release-gate dir>
Works: each run completes, its take lands and shows. Lean: what each machine fetched against what its runs loaded, 8-bit lanes and
kernels, nothing fetched twice, no disk refusal, disks of 300 GB (cl-1076), a quote's download no more than what landed (cl-1071).
Fast: rent, download and steps against bench.json (5%: a tie); a run's own work, from its GPU lease (a machine's first run: from the
executor start before it) to its end, against the stored run, which ran alone on a machine holding its weights: its download and its
wait for a GPU are judged by their own lines. Placed right: machine B works while A is busy, a run never waits for a busy machine's
GPU while a preferred one idles (cl-1073), a run takes every GPU of its machine. The page tells the truth: no page or console error; each take's tile names its phase (Downloading, then
Rendering) with that phase's share and its machine, and a cancel (cl-1074); no MBps, no "0 of 0"; no CLI command in a toast (cl-1085);
the take in Focus at its own shape (cl-1080); Calcifer never restarted; no page load nobody asked for (cl-1066); runs numbered from 1
in a new project (cl-1075). The H3 edit's steps are not all dense (cl-1081)."""
import json
import os
import re
import statistics as st
import sys
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import TypeAlias

FLOW, BENCH, GATE = sys.argv[1], sys.argv[2], sys.argv[3]
sys.path.insert(0, GATE)
from runs import JSON, Run  # noqa: E402

Obj: TypeAlias = dict[str, JSON]
TIE = 1.05
MIB, GB = 2**20, 1e9
PHASES = ('queued', 'installing', 'downloading', 'preparing', 'rendering', 'landing', 'completed')
# A run's own word for it, in its `waiting`: "waiting for a GPU, held by run <name>" (its stage still names its finished download).
GPU_WAIT = re.compile(r'waiting for (a|a free|the) GPU|let go of the GPUs', re.I)
verdict: list[str] = []


def check(name: str, ok: bool | None, detail: str) -> None:
    """None: nothing to judge it by, said and not failed."""
    verdict.append(f"{'PASS' if ok else 'NOTE' if ok is None else 'FAIL'} {name}: {detail}")


def obj(x: object) -> Obj:
    return x if isinstance(x, dict) else {}


def arr(x: object) -> list[JSON]:
    return x if isinstance(x, list) else []


def text(x: object) -> str:
    return x if isinstance(x, str) else ''


def num(x: object) -> float | None:
    return float(x) if isinstance(x, (int, float)) and not isinstance(x, bool) else None


def load(path: str) -> JSON:
    try:
        with open(os.path.join(FLOW, path)) as f:
            read: JSON = json.load(f)
            return read
    except (OSError, ValueError):
        return None


def lines(path: str) -> list[Obj]:
    out: list[Obj] = []
    try:
        with open(os.path.join(FLOW, path), errors='replace') as f:
            for line in f:
                try:
                    out.append(obj(json.loads(line)))
                except ValueError:
                    pass
    except OSError:
        pass
    return out


def secs(a: float | None, b: float | None) -> float | None:
    return None if a is None or b is None else (b - a) / 1000


def fmt(x: float | None, unit: str = '', nd: int = 0) -> str:
    return '?' if x is None else f"{x:.{nd}f}{unit}"


def eight(precision: str) -> bool:
    return bool(re.search(r'(int|fp|float)8', precision))


def pair(x: object) -> tuple[float, float]:
    """[w, h] as numbers, 0 for what's missing."""
    values = [num(v) or 0.0 for v in arr(x)] + [0.0, 0.0]
    return values[0], values[1]


@dataclass
class Machine:
    label: str
    record: Obj
    sku: str = ''
    gpus: int = 1
    name: str = ''


@dataclass
class Row:
    request: int
    box: str
    run: str
    target: str
    number: int | None
    sent_at: float
    machine: str = ''
    label: str = '?'
    status: str = 'never seen'
    reason: str = ''
    placed_s: float | None = None
    last_at: float | None = None
    first: dict[str, float] = field(default_factory=dict)
    # Where its stream placed it, in turn: (machine, from, to); a move starts another.
    spans: list[tuple[str, float, float]] = field(default_factory=list)
    # When its stream said it waited (queued, or for a GPU another run held) on a machine: (machine, from, to).
    waits: list[tuple[str, float, float]] = field(default_factory=list)
    # Its own GPU work: its calls' GPU leases, first start to last end, in ms; `work_s` to its end from there (see `own`).
    lease: tuple[float, float] | None = None
    work_s: float | None = None
    quoted_fetch: float | None = None
    dl_bytes: float = 0
    dl_s: float | None = None
    record: Run | None = None
    started_at: float | None = None
    settled_at: float | None = None
    exec_s: float | None = None
    dense: float | None = None
    sparse: float | None = None
    to_step1_s: float | None = None
    gpus: list[int] = field(default_factory=list)
    gemm: list[list[str]] = field(default_factory=list)
    vram_peak: int | None = None
    sol: dict[str, int] | None = None
    precisions: list[tuple[str, str, str]] = field(default_factory=list)
    used: set[str] = field(default_factory=set)
    fallbacks: list[str] = field(default_factory=list)
    stored: Obj = field(default_factory=dict)


@dataclass
class Held:
    label: str
    m: Machine
    held: list[tuple[str, float, list[str]]]
    fetched: float
    span: float | None
    twice: dict[str, list[int]]
    unused: list[tuple[str, float]]
    disk_gb: float | None
    free: float | None
    speed: str
    ops: list[Obj]


flow = obj(load('flow.json'))
with open(BENCH) as f:
    bench = obj(json.load(f))
renting = flow.get('rent') is True
events = lines('events.ndjson')
seen = lines('seen.ndjson')
machines: dict[str, Machine] = {}
for label, value in obj(flow.get('machines')).items():
    rec = obj(value)
    machines[label] = Machine(label, rec, text(rec.get('sku')), int(num(rec.get('gpus')) or 1), text(rec.get('name')))
by_name = {m.name: label for label, m in machines.items() if m.name}


def gone_at(m: Machine) -> float | None:
    """When it left by itself: gone from the page's list with End never pressed (a rental idle 15 min releases itself)."""
    if m.record.get('end_pressed_at'):
        return None
    return num(m.record.get('left_at')) or removed.get(m.name)

# Each run's changes as the page read them, and each machine's.
run_events: dict[str, list[tuple[float, Obj]]] = defaultdict(list)
machine_events: dict[str, list[tuple[float, Obj]]] = defaultdict(list)
removed: dict[str, float] = {}  # when each machine left the page's list
for e in events:
    at, ev = num(e.get('at')) or 0.0, obj(e.get('event'))
    if ev.get('type') == 'machine.removed':
        removed[text(ev.get('id'))] = at
    if ev.get('type') == 'run.updated':
        one = obj(ev.get('run'))
        run_events[text(one.get('id'))].append((at, one))
    elif ev.get('type') == 'machine.updated':
        one = obj(ev.get('machine'))
        machine_events[text(one.get('name'))].append((at, one))

for failure in arr(flow.get('failures')):
    check('flow', False, text(failure))
if not renting:
    check('pre-flight', not arr(flow.get('failures')), f"the page up to its quotes, nothing ordered: {', '.join(text(m.record.get('stated')) for m in machines.values())}")


def phase(run: Obj) -> str:
    """What a run is doing, from its record as the stream says it."""
    p = obj(run.get('progress'))
    stage = text(p.get('stage')).lower()
    status = text(run.get('status'))
    if status in ('completed', 'failed', 'canceled'):
        return 'landing' if run.get('arriving') else status
    if not run.get('machine') or run.get('queue_position') is not None or (run.get('waiting') and not p) or GPU_WAIT.search(text(run.get('waiting'))):
        return 'queued'
    if 'download' in stage or (num(p.get('bytes_total')) and not re.search(r'load|bake', stage)):
        return 'downloading'
    if re.search(r'install|environment', stage):
        return 'installing'
    if num(p.get('completed')) and num(p.get('total')) and not re.search(r'load|prepar|decode|encode', stage):
        return 'rendering'
    return 'preparing'


def own(full: Obj, cold: bool) -> tuple[float, float] | None:
    """Its calls' GPU leases (`execution.gpus[].start_us`/`end_us`), first start to last end, in ms. A machine's first run starts at the
    executor start those leases followed (its start-up is in the stored cold run's time too), when that came after the run started."""
    begins: list[float] = []
    ends: list[float] = []
    boots: list[float] = []
    for m in [obj(full.get('measurements'))] + [obj(obj(c).get('measurements')) for c in arr(full.get('calls'))]:
        ex = obj(m.get('execution'))
        gpus = [g for g in map(obj, arr(ex.get('gpus'))) if (num(g.get('gpu')) or 0.0) >= 0 and g.get('gpu') is not None]  # -1: an orchestrator, on no GPU
        begins += [v / 1000 for v in (num(g.get('start_us')) for g in gpus) if v]
        ends += [v / 1000 for v in (num(g.get('end_us')) for g in gpus) if v]
        boot = num(obj(ex.get('executor')).get('started_unix_ms'))
        if gpus and boot:
            boots.append(boot)
    if not begins or not ends:
        return None
    first = min(begins)
    if cold and boots and (num(full.get('started_at_ms')) or 0.0) <= min(boots) <= first:
        first = min(boots)
    return first, max(ends)


rows: list[Row] = []
for s in map(obj, arr(flow.get('sent'))):
    name = text(s.get('name')) or str(s.get('number'))
    number = num(s.get('number'))
    row = Row(int(num(s.get('request')) or 0), text(s.get('box')), name, text(s.get('target')), int(number) if number else None, num(s.get('at')) or 0.0)
    timeline = run_events.get(text(s.get('run')), [])
    for at, one in timeline:
        row.first.setdefault(phase(one), at)
    row.last_at = timeline[-1][0] if timeline else None
    for at, one in timeline:
        where = text(one.get('machine'))
        if where and (not row.spans or row.spans[-1][0] != where):
            if row.spans:
                row.spans[-1] = (row.spans[-1][0], row.spans[-1][1], at)
            row.spans.append((where, at, 0.0))
    waiting: tuple[str, float] | None = None
    for at, one in timeline:
        where = text(one.get('machine'))
        queued = bool(where) and phase(one) == 'queued'
        if waiting and (not queued or waiting[0] != where):
            row.waits.append((waiting[0], waiting[1], at))
            waiting = None
        if queued and not waiting:
            waiting = (where, at)
    if waiting and timeline:
        row.waits.append((waiting[0], waiting[1], timeline[-1][0]))
    placed = next((at for at, one in timeline if one.get('machine')), None)
    row.placed_s = secs(row.sent_at, placed)
    downloads = [(at, obj(one.get('progress'))) for at, one in timeline if phase(one) == 'downloading']
    row.dl_bytes = max((num(p.get('bytes_total')) or 0.0 for _, p in downloads), default=0.0)
    # The time its stream said it was downloading: not its wait for a GPU after, nor a later look at a download long done.
    took = sum(then - at for (at, one), (then, _) in zip(timeline, timeline[1:]) if phase(one) == 'downloading') / 1000
    row.dl_s = took if took > 0 else None
    rec_run = Run.read(os.path.join(FLOW, 'runs', f'{name}.json'))
    row.machine = (rec_run.record.get('machine') or '') if rec_run else (text(timeline[-1][1].get('machine')) if timeline else '')
    row.label = by_name.get(row.machine, '?')
    row.status = rec_run.status if rec_run else (text(timeline[-1][1].get('status')) if timeline else 'never seen')
    if rec_run:
        row.record, row.reason, row.target = rec_run, rec_run.reason, rec_run.target or row.target
        row.started_at, row.settled_at = num(rec_run.record.get('started_at_ms')), num(rec_run.record.get('settled_at_ms'))
        row.exec_s = rec_run.execution_ms / 1000 if rec_run.execution_ms else None
        steps = [x / 1000 for x in rec_run.steps]
        row.dense = st.median(steps[1:4]) if len(steps) >= 8 else None
        row.sparse = st.median(steps[4:8]) if len(steps) >= 8 else (st.median(steps) if steps else None)
        row.to_step1_s = rec_run.to_step1_ms / 1000 if rec_run.to_step1_ms else None
        row.gpus, row.gemm = rec_run.gpus, rec_run.gemm
        vram = rec_run.vram
        row.vram_peak = vram['peak'] if vram else None
        sol = [obj(o.get('fields')) for o in rec_run.observations if o.get('name') == 'attention.sol.calls']
        row.sol = {k: sum(int(num(f.get(k)) or 0) for f in sol) for k in ('dense_step', 'prefix_heavy', 'sparse')} if sol else None
        full = obj(json.loads(json.dumps(rec_run.record)))
        row.quoted_fetch = num(obj(full.get('placed')).get('fetch_bytes'))
        row.lease = own(full, not any(o.label == row.label and o.record for o in rows))
        if row.lease and row.settled_at:
            row.work_s = (row.settled_at - row.lease[0]) / 1000
        measured = [obj(part) for part in arr(obj(full.get('report')).get('measured'))]
        models = [obj(mm) for part in measured for mm in arr(part.get('models'))]
        row.precisions = [(text(mm.get('slot')), text(mm.get('lane')), text(mm.get('precision'))) for mm in models]
        row.used = {u.get('repository') or '' for u in rec_run.models} | {a.get('model') or '' for u in rec_run.models for a in u.get('adapters') or []}
        row.used |= {text(obj(a).get('model')) for mm in models for a in arr(mm.get('adapters'))}
        row.used.discard('')
        row.fallbacks = [f"{text(obj(c).get('component'))}: {obj(g).get('fallback')}" for part in measured for c in arr(part.get('components'))
                         for g in arr(obj(c).get('gemm')) if obj(g).get('fallback')]
    if row.spans:
        row.spans[-1] = (row.spans[-1][0], row.spans[-1][1], row.settled_at or row.last_at or 1e18)
    rows.append(row)

# ---- Works
for r in rows:
    check(f"completes request {r.request} ({r.run}, {r.box})", r.status == 'completed', f"{r.status} {r.reason}".strip())
marked = {text(obj(step).get('what')) for step in arr(flow.get('steps'))}
for box in sorted({r.box for r in rows}):
    got = f"{box}'s takes landed" in marked
    check(f'lands {box}', got, 'its takes are versions in the project' if got else 'not all landed')
focus = obj(flow.get('focus'))
if renting:
    video = obj(focus.get('video'))
    if video:
        (nw, nh), (sw, sh) = pair(video.get('natural')), pair(video.get('shown'))
        rw = pair(focus.get('region'))[0]
        stretch = abs((sw / sh) / (nw / nh) - 1) if nw and nh and sh else 1.0
        check('shows the take in Focus at its own shape (cl-1080)', nw > 0 and stretch < 0.03 and sw >= 0.4 * rw,
              f"{nw:.0f}x{nh:.0f} shown {sw:.0f}x{sh:.0f} in a {rw:.0f} px frame (shape off by {stretch:.1%})")
        poster = obj(focus.get('poster'))
        if poster:
            (pw, ph), (qw, qh) = pair(poster.get('natural')), pair(poster.get('shown'))
            check("Focus's still is the take's own picture, not a thumbnail stretched (cl-1080)", pw >= 0.5 * qw and ph >= 0.5 * qh,
                  f"{pw:.0f}x{ph:.0f} drawn at {qw:.0f}x{qh:.0f}")
    else:
        check('shows the take in Focus (cl-1080)', False, 'never reached')

calcifer = obj(flow.get('calcifer'))
pid0, pid1 = calcifer.get('pid_at_start'), calcifer.get('pid_at_end')
check('Calcifer never restarts', bool(pid0) and pid0 == pid1, f"pid {pid0} at start, {pid1} at end")
loads = [obj(x) for x in arr(flow.get('loads'))]
asked = sum(1 for x in loads if x.get('asked'))
unasked = len(loads) - 2 * asked
check('no page load nobody asked for (cl-1066)', unasked <= 1, f"{asked} asked, {unasked} more")
if renting:
    numbers = sorted(r.number for r in rows if r.number)
    check("a new project's runs are numbered from 1 (cl-1075)", bool(numbers) and numbers[-1] < 100, ', '.join(r.run for r in rows))

# ---- Lean, per machine: what landed (TensorD's `landed` lines), what it holds, what its runs loaded.
held_rows: list[Held] = []
for label, m in sorted(machines.items()):
    if not m.name:
        continue
    show = obj(load(f'machines/{label}.json'))
    status, rental = obj(show.get('status')), obj(show.get('rental'))
    log = ''
    try:
        with open(os.path.join(FLOW, 'machines', f'{label}-tensord.log'), errors='replace') as f:
            log = f.read()
    except OSError:
        pass
    landed: dict[str, list[int]] = defaultdict(list)
    stamps: list[float] = []
    for line in log.splitlines():
        if ' landed object=' not in line and ' fetching object=' not in line:
            continue
        t = re.match(r'(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?)Z', line)
        if t:
            stamps.append(datetime.fromisoformat(t.group(1)).replace(tzinfo=timezone.utc).timestamp())
        o, size = re.search(r'object=(\S+)', line), re.search(r'\bbytes=(\d+)', line)
        if ' landed object=' in line and o and size:
            landed[o.group(1)].append(int(size.group(1)))
    fetched = float(sum(sum(v) for v in landed.values()))
    twice = {o: v for o, v in landed.items() if len(v) > 1 and max(v) >= MIB}
    used: set[str] = set()
    for r in rows:
        if r.label == label:
            used |= r.used
    # `cozy machine show --json`: each model by its `repository`, the disk as `disk_total_bytes` and `disk_free_bytes`.
    held = [(text(obj(mm).get('repository')) or text(obj(mm).get('model')), num(obj(mm).get('total_bytes')) or 0.0,
             [text(obj(c).get('lane')) or '?' for c in arr(obj(mm).get('checkpoints'))]) for mm in arr(status.get('models'))]
    unused = [(model, size) for model, size, _ in held if model and not any(model == u or model.split('/')[-1] == u.split('/')[-1] for u in used)]
    disk = obj(status.get('disk'))
    speed = [f"{int(b) / float(sec) / 1e6:.0f} MB/s" for b, sec in re.findall(r"download speed to the Hub: Some\(WorkerDownload \{ bytes: (\d+), seconds: ([\d.]+)", log) if float(sec)]
    ops: dict[str, Obj] = {}
    for _, one in machine_events.get(m.name, []):
        for op in map(obj, arr(one.get('activity'))):
            if op.get('kind') == 'download':
                ops[text(op.get('id'))] = op
    disk_gb = num(rental.get('disk_gb')) or num(m.record.get('disk_gb'))
    h = Held(label, m, held, fetched, (max(stamps) - min(stamps)) if len(stamps) > 1 else None, twice, unused, disk_gb,
             num(status.get('disk_free_bytes')) or num(disk.get('free_bytes')),
             speed[-1] if speed else '', list(ops.values()))
    held_rows.append(h)
    if not landed:
        check(f'{label} fetched nothing twice', None, 'no TensorD log of its downloads read')
    else:
        check(f'{label} fetched nothing twice', not twice, ', '.join(f"{o[:19]} {len(v)}x {max(v) / MIB:.0f} MiB" for o, v in twice.items()) or f"{len(landed)} objects, {fetched / GB:.1f} GB")
    ran_here = [r for r in rows if r.label == label and r.record]
    check(f'{label} fetched nothing its runs did not load (cl-1077)', (bool(used) and not unused) if ran_here or held else None,
          ', '.join(f"{model} {size / GB:.1f} GB" for model, size in unused) or (f"holds only what its runs loaded: {', '.join(sorted(used))}" if ran_here else 'no request ran on it, and it holds nothing'))
    models_bytes = num(status.get('models_bytes')) or sum(size for _, size, _ in held)
    if models_bytes and landed:
        check(f'{label} fetched no more than it holds', fetched <= models_bytes * 1.02 + 64 * MIB, f"{fetched / GB:.1f} GB landed, {models_bytes / GB:.1f} GB held")
    check(f'{label} has a 300 GB disk (cl-1076)', (disk_gb or 0) >= 300, f"{disk_gb} GB")
    if renting:
        pressed, ended, went = num(m.record.get('end_pressed_at')), num(m.record.get('ended_at')), gone_at(m)
        if went:
            idle_from = max([r.settled_at or 0.0 for r in ran_here] + [num(m.record.get('ready_at')) or 0.0])
            check(f'{label} ended through the page', None,
                  f"it ended by itself at {datetime.fromtimestamp(went / 1000, timezone.utc):%H:%M:%S}, before End was pressed, {fmt(secs(idle_from, went), ' s')} after "
                  + (f"its last run settled (requests {[r.request for r in ran_here]})" if ran_here else 'it was ready: no request ran on it') + '; a rental idle 15 min releases itself')
        elif pressed:
            check(f'{label} ended through the page', bool(ended), f"gone from the list {fmt(secs(pressed, ended), ' s')} after End" if ended else 'End pressed, never gone from the list')

for r in rows:
    if not r.record:
        continue
    low = [f"{slot} {lane} ({precision})" for slot, lane, precision in r.precisions if 'lora' not in slot and not eight(precision)]
    check(f"request {r.request} runs 8-bit lanes (cl-1076)", bool(r.precisions) and not low, ', '.join(low) or ', '.join(f"{s} {l}" for s, l, _ in r.precisions))
    wide = [f"{f} {k}" for f, k in r.gemm if not eight(f)]
    check(f"request {r.request} serves 8-bit GEMM kernels", bool(r.gemm) and not wide and not r.fallbacks,
          ', '.join(wide + r.fallbacks) or ', '.join(f"{f} {k}" for f, k in r.gemm))
    said = f"{r.status} {r.reason} {json.dumps(obj(json.loads(json.dumps(r.record.record))).get('warnings') or [])}"
    refused = re.search(r'disk|no room|space', said, re.I)
    check(f"request {r.request} met no disk refusal", not refused, said[:200] if refused else 'none')

    # What its quote said it would fetch against its own download, where it fetched (cl-1071: quoted 65.1 GiB, 22 GiB landed).
    if r.quoted_fetch and r.dl_bytes and not any(o.label == r.label and o.request < r.request for o in rows):
        check(f"request {r.request}'s quote fetches what its run does (cl-1071)", r.quoted_fetch <= r.dl_bytes * 1.25 + GB,
              f"quoted {r.quoted_fetch / GB:.1f} GB, its download {r.dl_bytes / GB:.1f} GB")

# ---- Fast: against bench.json, 5% a tie.
for r in rows:
    if not r.record:
        continue
    fn = re.sub(r'^.*?([^/]+/[^/@]+)(@.*)?$', r'\1', r.target)
    mm = machines.get(r.label)
    key = f"{mm.sku if mm else '?'}x{len(r.gpus) or (mm.gpus if mm else 1)}"
    r.stored = obj(obj(bench.get(fn)).get(key))
    if not r.stored:
        check(f"request {r.request} as fast as stored", None, f"no stored {fn} on {key}: recorded; its first green numbers go in bench.json")
        continue
    cold = not any(o.label == r.label and o.request < r.request and o.record for o in rows)
    want = num(r.stored.get('cold_exec_s' if cold else 'warm_exec_s')) or 0.0
    # Its own work, as the stored run's time is: that run was alone on a machine that held its weights. This one's download and its wait
    # for a GPU behind another run are not its work; its download's rate and placement judge those.
    check(f"request {r.request}'s own work within 5% of stored ({fn}, {key}, {'cold' if cold else 'warm'})", r.work_s is not None and r.work_s <= want * TIE,
          f"{fmt(r.work_s, ' s', 1)} vs {want} s" + ('' if r.work_s is not None else ': no GPU lease in its record'))
    for part, step in (('dense', r.dense), ('sparse', r.sparse)):
        stored = num(r.stored.get(f'{part}_step_s'))
        if stored and step is not None:
            check(f"request {r.request} {part} steps within 5% of stored ({key})", step <= stored * TIE, f"{step:.1f} s vs {stored} s")
for r in rows:
    if r.record and 'edit' in r.target:
        sparse = (r.sol or {}).get('sparse', 0)
        check(f"request {r.request}'s edit runs sparse steps (cl-1081)", sparse > 0, f"attention.sol.calls {r.sol}" if r.sol else 'no attention.sol.calls observed')
for h in held_rows:
    boot = secs(num(h.m.record.get('ordered_at')), num(h.m.record.get('ready_at')))
    check(f"{h.label} boots within 15 min", boot is not None and boot <= 900, f"ordered to ready {fmt(boot, ' s')}")
    for r in rows:
        if r.label == h.label and r.dl_bytes and r.dl_s:
            rate = r.dl_bytes / r.dl_s / 1e6
            check(f"request {r.request}'s download at 50 MB/s or more", rate >= 50, f"{r.dl_bytes / GB:.1f} GB in {r.dl_s:.0f} s, {rate:.0f} MB/s; link {h.speed or 'not reported'}")

# ---- Placed right (cl-1073, its GPU-only parts proven here). The walk's own words (walk.log, calcifer.diagnostics): each take it placed
# or moved, every machine's quote beside it; Calcifer's log for each rental's move to the RC (an update); the page's stream for where each
# run stood when.
@dataclass
class Decision:
    at: float
    key: str
    to: str
    moved_from: str
    offers: dict[str, float | None]
    line: str


def iso_ms(stamp: str) -> float | None:
    t = re.match(r'(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(\.\d+)?Z', stamp)
    if not t:
        return None
    return (datetime.strptime(t.group(1), '%Y-%m-%dT%H:%M:%S').replace(tzinfo=timezone.utc).timestamp() + float('0' + (t.group(2) or '.0'))) * 1000


def read_text(path: str) -> str:
    try:
        with open(os.path.join(FLOW, path), errors='replace') as f:
            return f.read()
    except OSError:
        return ''


decisions: list[Decision] = []
for line in read_text('walk.log').splitlines():
    w = re.match(r'^(\S+Z) (.+?) (?:goes to (\S+)|moves from (\S+) to (\S+)): (.*)$', line)
    when = iso_ms(w.group(1)) if w else None
    if not w or when is None:
        continue
    offers: dict[str, float | None] = {o.group(1): float(o.group(2)) for o in re.finditer(r"(\S+) finishes in (\d+) s \(", w.group(6))}
    offers |= {o.group(1): None for o in re.finditer(r"(\S+) can't: ", w.group(6))}
    decisions.append(Decision(when, w.group(2), w.group(3) or w.group(5) or '', w.group(4) or '', offers, line[:400]))
name_of = {m.name: label for label, m in machines.items() if m.name}


def held_on(name: str, at: float, but: int = 0) -> list[tuple[Row, float]]:
    """Our runs on machine `name` at `at` (placed there and not yet gone or settled), each with when it left it."""
    return [(o, until) for o in rows if o.request != but for where, since, until in o.spans if where == name and since <= at < until]


def request_of(d: Decision) -> int:
    """The request a decision is about: placed (or moved) where it says within a minute of it."""
    near = [(abs(since - d.at), o.request) for o in rows for where, since, _ in o.spans if where == d.to and abs(since - d.at) <= 60_000]
    return min(near)[1] if near else 0


updates: list[tuple[str, float, float | None]] = []  # (machine, began, ready again)
for line in read_text('calcifer.log').splitlines():
    if 'to the open RC' not in line:
        continue
    began, who = iso_ms(line), re.search(r'machine="?([^"\s]+)', line)
    if began is None or not who:
        continue
    after = [t for t, one in machine_events.get(who.group(1), []) if t >= began and '.dev' in text(one.get('runtime'))]
    updates.append((who.group(1), began, min(after) if after else None))

if renting:
    done = [r for r in rows if r.started_at and r.settled_at]
    overlap = [(a.request, b.request) for a in done for b in done if a.label == 'A' and b.label == 'B'
               and (b.started_at or 0) < (a.settled_at or 0) and (a.started_at or 0) < (b.settled_at or 0)]
    check('B works while A is busy (cl-1073)', bool(overlap), f"overlapping requests (A, B): {overlap}" if overlap else f"runs: {[(r.request, r.label) for r in done]}")
    check('the walk said where it placed each take (walk.log)', bool(decisions), f"{len(decisions)} decisions" if decisions else 'no walk.log decisions read')

    # A busy machine's quote counts what its runs ahead still take: never "all but done" (cause 1: 6 s quoted, 528 s left).
    short, judged = [], 0
    for d in decisions:
        for name, quoted in d.offers.items():
            ahead = held_on(name, d.at, request_of(d))
            if quoted is None or not ahead or name not in name_of:
                continue
            judged += 1
            left = (max(until for _, until in ahead) - d.at) / 1000
            if quoted < 0.5 * left and left - quoted > 120:
                short.append(f"{datetime.fromtimestamp(d.at / 1000, timezone.utc):%H:%M:%S} {d.key}: {name} quoted {quoted:.0f} s, its runs ahead held it {left:.0f} s more")
    check("a busy machine's quote counts its runs' time left (cl-1073)", (not short) if judged else None,
          '; '.join(short[:3]) or (f"{judged} quotes of a busy machine, each at least half its runs' real time left" if judged else 'no quote of a busy machine to judge'))

    # A new request goes to the free machine while the other is mid-run or downloading: request 3, sent while A holds 1 and 2 and B boots.
    r3 = next((r for r in rows if r.request == 3), None)
    if r3 and machines.get('B') and machines['B'].name:
        busy_a = held_on(machines['A'].name, r3.sent_at, 3) if machines.get('A') else []
        placing = next((d.line for d in decisions if request_of(d) == 3), '')
        check('a new request goes to the free machine while the other is busy (cl-1073)', r3.label == 'B' if busy_a else None,
              f"request 3 ran on {r3.label} {r3.machine}, A holding {[o.request for o, _ in busy_a]}; {placing}" if busy_a else 'A was free when request 3 was sent')

    # A run waiting on a busy machine moves to one that finishes it clearly sooner (cause 3: an LTX run waited 40 min). Waiting is the
    # run's own word in the page's stream (`waits`: queued, or "waiting for a GPU, held by run <name>"); its `started_at_ms` is no end of
    # it, since a run starts on its machine before its download and its turn on the GPU. A machine sits free from when it's ready and
    # preferred until it leaves, but while one of our runs is placed on it.
    moved = [d for d in decisions if d.moved_from]
    worst, why = 0.0, ''
    for r in rows:
        for where, since, until in r.waits:
            for label, m in machines.items():
                free_from = max(num(m.record.get('preferred_at')) or 1e18, num(m.record.get('ready_at')) or 1e18)
                free_to = min(gone_at(m) or 1e18, num(m.record.get('end_pressed_at')) or 1e18)
                if not m.name or m.name == where:
                    continue
                # The longest stretch of this wait with nothing of ours on `m`.
                edges = sorted({max(since, free_from), min(until, free_to)} | {t for o in rows for w, a, z in o.spans if w == m.name for t in (a, z) if since < t < until})
                for a, z in zip(edges, edges[1:]):
                    if z - a > worst * 1000 and a >= free_from and z <= free_to and not held_on(m.name, (a + z) / 2):
                        worst, why = (z - a) / 1000, f"request {r.request} waited on {where} ({name_of.get(where, '?')}) while {m.name} ({label}) sat free"
    if moved:
        check('a waiting run moves to a machine clearly sooner (cl-1073)', worst <= 120,
              '; '.join(f"request {request_of(d)} {d.key} moved from {d.moved_from} to {d.to}: {d.line.split(': ', 1)[-1][:200]}" for d in moved)
              + (f"; but {why} {worst:.0f} s" if worst > 120 else ''))
    else:
        check('a waiting run moves to a machine clearly sooner (cl-1073)', False if worst > 120 else None,
              f"{why} for {worst:.0f} s, never moved" if worst > 120 else 'no run waited on a busy machine while another was free')

    # A machine updating (a dev home's move to the RC) isn't free: its quote waits for its update (cause 2: held 20 min "getting ready").
    early, judged = [], 0
    for name, began, ready_again in updates:
        for d in decisions:
            quoted = d.offers.get(name)
            if ready_again is None or quoted is None or not began <= d.at < ready_again:
                continue
            judged += 1
            if quoted < (ready_again - d.at) / 1000 - 30:
                early.append(f"{d.key}: {name} quoted {quoted:.0f} s while its update took {(ready_again - d.at) / 1000:.0f} s more")
    check("a machine updating isn't free (cl-1073)", (not early) if judged else None,
          '; '.join(early[:3]) or (f"{judged} quotes during {len(updates)} updates, each after its update" if judged else f"{len(updates)} updates, no quote during one"))

for label, m in machines.items():
    on = [r for r in rows if r.label == label and r.started_at and r.settled_at]
    if m.gpus <= 1 or not on:
        continue
    for r in on:
        check(f"request {r.request} takes every GPU of {label} (cl-1073)", len(r.gpus) == m.gpus, f"GPUs {r.gpus} of {m.gpus}")
    # By their GPU leases: two runs both started on the machine while one waits for the other's GPUs.
    leased = [(r.request, r.lease) for r in on if r.lease]
    split = [(a, b) for a, x in leased for b, y in leased if a < b and y[0] < x[1] and x[0] < y[1]]
    check(f"the next run on {label} queues or goes elsewhere, never beside one on its GPUs (cl-1073)", (not split) if leased else None,
          f"ran at once: {split}" if split else f"{len(leased)} runs, one at a time" if leased else 'no GPU lease in their records')

# ---- The page tells the truth.
tiles: dict[str, list[Obj]] = defaultdict(list)
for s in seen:
    if text(s.get('key')).startswith('tiles '):
        try:
            for tile in map(obj, arr(json.loads(text(s.get('text'))))):
                if tile.get('making'):
                    tiles[text(tile.get('run')) or text(tile.get('title'))].append(tile)
        except ValueError:
            pass
if renting:
    for r in rows:
        said_tiles = tiles.get(str(r.number)) or tiles.get(f"Run {r.run}") or []
        if not said_tiles:
            check(f"request {r.request}'s tile seen in Focus", None, 'its box was not in Focus while it rendered')
            continue
        phases = [text(t.get('phase')) for t in said_tiles if t.get('phase')]
        order = [p for i, p in enumerate(phases) if i == 0 or p != phases[i - 1]]
        ok = 'Rendering' in order and (not r.dl_bytes or ('Downloading' in order and order.index('Downloading') < order.index('Rendering')))
        check(f"request {r.request}'s tile says Downloading, then Rendering (cl-1074)", ok, ' > '.join(order) or f"no phase named: {text(said_tiles[-1].get('text'))!r}")
        shares = [t for t in said_tiles if t.get('phase') in ('Downloading', 'Rendering')]
        check(f"request {r.request}'s tile gives each phase its share (cl-1074)", bool(shares) and all(re.fullmatch(r'\d{1,3}%', text(t.get('share'))) for t in shares),
              f"{len(shares)} looks")
        named = sorted({text(t.get('where')) for t in said_tiles if t.get('where')})
        check(f"request {r.request}'s tile names its machine (cl-1074)", bool(r.machine) and any(r.machine in w for w in named), f"said {named}, ran on {r.machine}")
        check(f"request {r.request}'s tile has its own cancel (cl-1074)", all(t.get('cancel') for t in said_tiles),
              f"{sum(1 for t in said_tiles if t.get('cancel'))} of {len(said_tiles)} looks")
if 'errors' in flow:
    errors = [text(obj(e).get('line')) for e in arr(flow.get('errors'))]
    check('the page reports no error (page and console errors)', not errors, f"{len(errors)}: " + '; '.join(errors[:3]) if errors else 'none')
rates = [text(s.get('text')) for s in seen if s.get('key') == 'MBps']
check('the page says rates in MB/s, never MBps (cl-1074)', not rates, '; '.join(rates[:3]) or 'never MBps')
zeros = [text(s.get('text')) for s in seen if s.get('key') == '0 of 0']
check('the page never says "0 of 0" (cl-1074)', not zeros, '; '.join(zeros[:3]) or 'never')
toasts = [text(s.get('text')) for s in seen if text(s.get('key')).startswith('note ')]
commands = [t for t in toasts if re.search(r'\bcozy (open|run|machine|model|package|vault|login|update|restart|new|status)\b', t)]
check('no toast shows a CLI command (cl-1085)', not commands, '; '.join(commands[:3]) or f"{len(toasts)} toasts")

# ---- Tables.
cost = [obj(c) for c in arr(load('cost.json'))]
spent = sum(num(c.get('spend_usd_micros')) or 0.0 for c in cost) / 1e6
each = ', '.join('%s %s: $%.2f' % (text(c.get('rental_id')), text(c.get('sku')), (num(c.get('spend_usd_micros')) or 0.0) / 1e6) for c in cost) or 'not read'
tools = ', '.join(text(obj(t).get('id')) + '@' + text(obj(t).get('version')) for t in arr(flow.get('tools'))) or '?'
fails = [v for v in verdict if v.startswith('FAIL')]
out = [f"### user-flow: {'PASS' if not fails else 'FAIL'} ({len(fails)} of {sum(1 for v in verdict if not v.startswith('NOTE'))} checks failed)\n",
       f"cozy {text(flow.get('cozy')) or '?'}; A {text(flow.get('sku'))} x1, B {text(flow.get('sku_b'))} x{flow.get('gpus_b')}; tools {tools}; billed ${spent:.2f} ({each})\n",
       '\n#### Each run\n',
       '| # | run | box | function | machine | sent → placed | downloaded | waited (queue, GPU) | to step 1 | dense / sparse step (stored) | own work (stored cold / warm) | sent → done | lanes | GEMM | GPUs | VRAM peak | status |',
       '|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|']
for r in rows:
    dl = f"{r.dl_bytes / GB:.1f} GB in {fmt(r.dl_s, ' s')}" if r.dl_bytes else '-'
    stepped = f"{fmt(r.dense, ' s', 1)} / {fmt(r.sparse, ' s', 1)} ({r.stored.get('dense_step_s', '-')} / {r.stored.get('sparse_step_s', '-')})"
    lanes = ', '.join(f"{s} {l}" for s, l, _ in r.precisions) or '?'
    gemm = ', '.join(f for f, _ in r.gemm) or '?'
    peak = f"{r.vram_peak / 1024:.1f} GiB" if r.vram_peak else '?'
    waited = sum(until - since for _, since, until in r.waits) / 1000
    out.append(f"| {r.request} | {r.run} | {r.box} | {r.target.split('/')[-1] or '?'} | {r.label} {r.machine} | {fmt(r.placed_s, ' s')} | {dl} | {waited:.0f} s | "
               f"{fmt(r.to_step1_s, ' s')} | {stepped} | {fmt(r.work_s, ' s')} ({r.stored.get('cold_exec_s', '-')} / {r.stored.get('warm_exec_s', '-')}) | {fmt(secs(r.sent_at, r.last_at), ' s')} | "
               f"{lanes} | {gemm} | {r.gpus or '?'} | {peak} | {r.status} |")
out += ['\n#### Each machine\n',
        '| machine | SKU x GPUs | stated | ordered → ready | disk | holds | landed | fetched twice | unused | link | ended |',
        '|---|---|---|---|---|---|---|---|---|---|---|']
for h in held_rows:
    rec = h.m.record
    pace = f" ({h.fetched / h.span / 1e6:.0f} MB/s)" if h.span else ''
    holds = '; '.join(f"{model} {size / GB:.1f} GB {'/'.join(lanes)}" for model, size, lanes in h.held) or '?'
    out.append(f"| {h.label} {h.m.name} | {h.m.sku} x{h.m.gpus} | {text(rec.get('stated')) or '?'} | {fmt(secs(num(rec.get('ordered_at')), num(rec.get('ready_at'))), ' s')} | "
               f"{fmt(h.disk_gb, ' GB')}, {fmt((h.free or 0.0) / GB, ' GB free')} | {holds} | {h.fetched / GB:.1f} GB in {fmt(h.span, ' s')}{pace} | "
               f"{len(h.twice)} | {', '.join(x for x, _ in h.unused) or '-'} | {h.speed or '?'} | "
               + (f"by itself, {datetime.fromtimestamp((gone_at(h.m) or 0.0) / 1000, timezone.utc):%H:%M:%S} |" if gone_at(h.m) else f"{fmt(secs(num(rec.get('end_pressed_at')), num(rec.get('ended_at'))), ' s after End')} |"))
out += ["\n#### Download events: each machine's weights, as the page read them\n",
        '| machine | what | fetched | already there | started | stage | rate |', '|---|---|---|---|---|---|---|']
for h in held_rows:
    for op in h.ops:
        started = num(op.get('started_at_ms'))
        clock = datetime.fromtimestamp(started / 1000, timezone.utc).strftime('%H:%M:%S') if started else '?'
        out.append(f"| {h.label} | {text(op.get('subject'))} | {(num(op.get('bytes_done')) or 0.0) / GB:.2f} / {(num(op.get('bytes_total')) or 0.0) / GB:.2f} GB | "
                   f"{(num(op.get('bytes_here')) or 0.0) / GB:.2f} GB | {clock} | {text(op.get('stage'))} | {fmt((num(op.get('bytes_per_s')) or 0.0) / 1e6, ' MB/s')} |")
out += ["\n#### Inference events: each run's phases, seconds after it was sent\n", '| # | run | ' + ' | '.join(PHASES) + ' |', '|---|---|' + '---|' * len(PHASES)]
for r in rows:
    out.append(f"| {r.request} | {r.run} | " + ' | '.join(fmt(secs(r.sent_at, r.first.get(p))) for p in PHASES) + ' |')
out += ['\n#### Checks\n'] + [f"- {v}" for v in verdict]
print('\n'.join(out))
with open(os.path.join(FLOW, 'verdict.txt'), 'w') as f:
    f.write('\n'.join(verdict) + '\n')
sys.exit(1 if fails else 0)
