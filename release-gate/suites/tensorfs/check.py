"""Suite `tensorfs`'s verdict, from what cells.sh kept under <gate>/suite-tensorfs (analyze.py runs check(gate) when
the suite is selected). Times are held against the SKU's last passed suite (baseline['suites']['tensorfs']), like the
product cells: the worst known-good plus 25%, unless the known-good spread is wider. Rates are reported beside the
probe's disk ceiling; an executor never dials, so there is no network ceiling to read them against. A PASS appends
this run to the baseline.
    check.py --past <ndjson> <stage> <fraction>   bytes moved once <stage> passed the fraction (cells.sh's tfs_cut)
    check.py --dropped <ndjson> <stage>   bytes moved when the machine went: progress lost, or a wait after it began; none if it never did"""
import json
import os
import sys
import time
from collections.abc import Callable
from typing import NotRequired, TypedDict, cast

SLACK = 0.25

type JSON = str | int | float | bool | None | list[JSON] | dict[str, JSON]


class Progress(TypedDict, total=False):
    stage: str
    bytes_done: int
    bytes_total: int


class Settled(TypedDict, total=False):
    settled_at_ms: int


class Line(TypedDict, total=False):
    """A line of a run's events (`type`, ...) or of straggler.py's proxy log (`kind`, ...)."""
    type: str
    progress: Progress
    at_ms: int
    text: str
    run: Settled
    kind: str
    path: str
    slow: bool
    ask: int
    bytes: int
    silent: bool


class RunRecord(TypedDict, total=False):
    status: str
    reason: JSON
    result: JSON


class Bytes(TypedDict, total=False):
    """A result's `bytes: {hub, volume, local}`."""
    hub: int
    volume: int
    local: int


class Measured(TypedDict, total=False):
    """The probe's result (ci_tfs_probe.Measured)."""
    disk_write_bytes_per_s: int
    disk_read_bytes_per_s: int


class Held(TypedDict):
    component: str
    key: str
    scheme: str | None
    relative_error: float | None


class Inspected(TypedDict, total=False):
    """multi-conversion's `inspect` result (ci_multi_conversion.Report)."""
    checkpoint: str
    tensors: list[Held]


class LoraConfig(TypedDict, total=False):
    peft_type: str
    lora_alpha: float
    r: int


class Row(TypedDict, total=False):
    run: str
    wall_s: float | None
    rec: RunRecord | None
    evs: list[Line]


class Straggler(TypedDict):
    ok: bool
    seconds: float
    slowed: int
    reasked: int
    bytes: int
    asks: int
    error: str
    silenced: set[str]
    body_failed: int


class Timing(TypedDict):
    seconds: float
    bytes: int


class KnownGood(Timing):
    label: str
    at: str


class Suites(TypedDict, total=False):
    tensorfs: dict[str, list[KnownGood]]


class Baseline(TypedDict):
    suites: NotRequired[Suites]


class Gate(TypedDict):
    out: str
    baseline: Baseline


def events(path: str) -> list[Line]:
    out: list[Line] = []
    for line in open(path) if os.path.exists(path) else ():
        line = line.strip()
        if line.startswith('{'):
            try:
                out.append(json.loads(line))
            except ValueError:
                pass
    return out


def stage_progress(evs: list[Line], stage: str) -> tuple[int, int]:
    """(bytes done, bytes total) of the last progress in a stage naming <stage>."""
    last = (0, 0)
    for e in evs:
        p = e.get('progress') or {}
        if e.get('type') == 'run.progress' and stage in (p.get('stage') or ''):
            last = (p.get('bytes_done') or 0, p.get('bytes_total') or 0)
    return last


def dropped(evs: list[Line], stage: str) -> int | None:
    """The bytes <stage> had moved when the machine went: a wait after progress began, or progress lost. None if it never did."""
    last = 0
    for e in evs:
        p = e.get('progress') or {}
        if e.get('type') == 'run.progress' and stage in (p.get('stage') or ''):
            if (p.get('bytes_done') or 0) < last:
                return last
            last = p.get('bytes_done') or 0
        elif e.get('type') == 'run.waiting' and last:
            return last
    return None


def stage_seconds(evs: list[Line], stage: str) -> float | None:
    """From the log line that opened <stage> to the run's end."""
    start = next((e['at_ms'] for e in evs if e.get('type') == 'run.log' and stage in (e.get('text') or '')), None)
    end = next((e['run'].get('settled_at_ms') for e in reversed(evs) if isinstance(e.get('run'), dict)
                and e['run'].get('settled_at_ms')), None)
    return (end - start) / 1000 if start and end else None


def hub_bytes(rec: RunRecord | None) -> tuple[int | None, int | None]:
    """What the run fetched from the Hub and the volume: every `bytes: {hub, volume}` its result carries."""
    found = landed_bytes(rec)
    return (sum(b.get('hub') or 0 for b in found), sum(b.get('volume') or 0 for b in found)) if found else (None, None)


def landed_bytes(rec: RunRecord | None) -> list[Bytes]:
    """Every `bytes: {hub, volume, local}` the run's result carries."""
    found: list[Bytes] = []

    def walk(o: JSON) -> None:
        if isinstance(o, dict):
            b = o.get('bytes')
            if isinstance(b, dict) and 'hub' in b:
                found.append(cast(Bytes, b))  # cast: a result's `bytes` object holding `hub` is the store's {hub, volume, local}
            for v in o.values():
                walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)
    walk((rec or {}).get('result'))
    return found


def check(gate: Gate) -> tuple[list[str], list[str]]:
    t = os.path.join(gate['out'], 'suite-tensorfs')
    if not os.path.isdir(t):
        return [], []
    fails: list[str] = []
    notes: list[str] = []
    rows: dict[str, Row] = {}
    for line in open(os.path.join(t, 'steps.tsv')) if os.path.exists(os.path.join(t, 'steps.tsv')) else ():
        name, number, wall_ms = (line.rstrip('\n').split('\t') + ['', ''])[:3]
        p = os.path.join(t, name + '.json')
        rows[name] = {'run': number, 'wall_s': int(wall_ms) / 1000 if wall_ms else None,
                      'rec': json.load(open(p)) if os.path.exists(p) else None, 'evs': events(os.path.join(t, name + '.ndjson'))}

    # The cases cells.sh ran (--only selects some); none listed: every case.
    path = os.path.join(t, 'cases')
    cases = set(open(path).read().split()) if os.path.exists(path) else None

    def chosen(name: str) -> bool:
        case = name.split('-')[0]
        return cases is None or {'mc': 'multi-conversion', 'lora': 'lora-alpha', 'layout': 'family'}.get(case, case) in cases

    def done(name: str, cancelled: bool = False) -> bool:
        if not chosen(name):
            return False
        r = rows.get(name)
        status = ((r or {}).get('rec') or {}).get('status')
        if r is None:
            fails.append(f'tensorfs {name}: did not run')
            return False
        if cancelled:
            return True
        if status != 'completed':
            why = ((r or {}).get('rec') or {}).get('reason')
            fails.append(f'tensorfs {name}: run {(r or {}).get("run")} {status} {json.dumps(why)[:300] if why else ""}')
            return False
        return True

    base = gate['baseline'].setdefault('suites', {}).setdefault('tensorfs', {})
    now: dict[str, Timing] = {}

    def against(case: str, seconds: float | None, nbytes: int, what: str) -> None:
        if seconds is None:
            return
        now[case] = {'seconds': round(seconds, 2), 'bytes': nbytes}
        good = [g for g in base.get(case, []) if not nbytes or not g.get('bytes') or abs(g['bytes'] - nbytes) <= 0.05 * nbytes]
        if not good:
            notes.append(f'tensorfs {case}: {what} {seconds:.1f} s, no known-good yet: this run starts them')
            return
        vals = [g['seconds'] for g in good]
        limit = max(vals) + max(SLACK * max(vals), max(vals) - min(vals))
        (fails if seconds > limit else notes).append(
            f'tensorfs {case}: {what} {seconds:.1f} s vs known-good {min(vals):.1f}-{max(vals):.1f} s (limit {limit:.1f})')

    if done('probe'):
        probe = rows['probe']['rec']
        assert probe is not None
        d = cast(Measured, probe.get('result') or {})  # cast: the probe's result is its Measured
        notes.append(f'tensorfs probe: the machine\'s disk writes {(d.get("disk_write_bytes_per_s") or 0) / 1e6:.0f} MB/s, '
                     f'reads cold {(d.get("disk_read_bytes_per_s") or 0) / 1e6:.0f} MB/s')
    st: dict[str, Straggler] = {}
    for line in open(os.path.join(t, 'straggler.tsv')) if os.path.exists(os.path.join(t, 'straggler.tsv')) else ():
        name, code, ms = line.rstrip('\n').split('\t')
        proxy = events(os.path.join(t, name + '.proxy'))
        objs = [r for r in proxy if r.get('kind') == 'object']
        slow = {r['path'] for r in objs if r['slow']}
        st[name] = {'ok': code == '0', 'seconds': int(ms) / 1000, 'slowed': len(slow),
                    'reasked': sum(1 for r in objs if r['ask'] > 1), 'bytes': sum(r['bytes'] for r in objs),
                    'asks': max((r['ask'] for r in objs if r['path'] in slow), default=0),
                    'error': open(os.path.join(t, name + '.err')).read()[-300:] if code != '0' else '',
                    'silenced': {r['path'] for r in objs if r.get('silent')},
                    'body_failed': sum('body failed' in line for line in open(os.path.join(t, name + '.err'), errors='replace'))}
    c = st.get('straggler-clean')
    # Each slowed arm against the clean one: stragglers (the first ask drips), cold objects (every ask drips, tfs-138),
    # crawling objects (every ask moves 64 KiB and drips, tfs-141), and many, whose parts come at full speed (tfs-143).
    # Judged against fixed bars, whatever the downloader's range size (lead, 11-05): a range size that moved the bar or
    # started a fresh series let 16 MiB ranges pass cold objects at 234 s and crawling ones at 263 s, against 16 s and 24 s
    # on the downloader before (41920882049).
    for name, case, what in (('straggler-slow', 'straggler', 'stragglers'), ('straggler-cold', 'straggler-cold', 'cold objects'),
                             ('straggler-crawl', 'straggler-crawl', 'crawling objects'),
                             ('straggler-parts', 'straggler-parts', 'parts at full speed')):
        sl = st.get(name)
        if not chosen('straggler'):
            break
        if not c or not sl:
            fails.append(f'tensorfs {case}: the case did not run')
        elif not c['ok'] or not sl['ok']:
            fails.append(f'tensorfs {case}: tfs download failed (clean: {c["error"]}; with {what}: {sl["error"]})')
        elif not sl['slowed']:
            fails.append(f'tensorfs {case}: no object was slowed: the case tested nothing')
        else:
            # Crawling objects pay their parts' drip through this proxy; crawled through, they take over 5 min.
            limit = 2 * c['seconds'] + 60 if name in ('straggler-crawl', 'straggler-parts') else 1.5 * c['seconds'] + 20
            (fails if sl['seconds'] > limit else notes).append(
                f'tensorfs {case}: {sl["slowed"]} slow objects, {sl["reasked"]} re-asked, at most {sl["asks"]} asks of one; '
                f'{sl["seconds"]:.1f} s vs clean {c["seconds"]:.1f} s (limit {limit:.1f})')
            against(case, sl['seconds'], c['bytes'], f'download with {what}')
    # Arms that must land: through one 5 MB/s link for every request (run 31103: the old downloader moved nothing for good
    # on one), with every object's partial held by another process (gate 41991226171: IO_FAILED, Bad file descriptor), and
    # through 8 s of 503s once the ramp has settled (run 32321: one request left, held by nothing to start).
    for name, what in (('straggler-link', 'through a 5 MB/s link'), ('straggler-held', "with every object's partial held elsewhere"),
                       ('straggler-burst', 'through a burst of 503s')):
        sl = st.get(name)
        if not chosen('straggler'):
            break
        if not sl:
            fails.append(f'tensorfs {name}: the case did not run')
        elif not sl['ok']:
            fails.append(f'tensorfs {name}: tfs download {what} failed: {sl["error"]}')
        else:
            notes.append(f'tensorfs {name}: {sl["bytes"] / 2**20:.0f} MiB {what} in {sl["seconds"]:.1f} s '
                         f'({sl["bytes"] / sl["seconds"] / 1e6:.1f} MB/s)')

    # One small ask's socket held silent after half its body, the last range left (run 32321 sat 16 min on its last two):
    # the no-bytes stall ends it and it is asked again from the byte reached, within 60 s.
    sl = st.get('straggler-silent')
    if chosen('straggler'):
        if not sl:
            fails.append('tensorfs straggler-silent: the case did not run')
        elif not sl['ok']:
            fails.append(f'tensorfs straggler-silent: tfs download with a silent socket failed: {sl["error"]}')
        elif not sl['silenced']:
            fails.append('tensorfs straggler-silent: no ask was held silent: the case tested nothing')
        else:
            asks = max((r['ask'] for r in events(os.path.join(t, 'straggler-silent.proxy')) if r.get('path') in sl['silenced']), default=0)
            good = sl['seconds'] <= 60 and sl['body_failed'] >= 1 and asks >= 2
            (notes if good else fails).append(f'tensorfs straggler-silent: {sl["seconds"]:.1f} s (limit 60), {sl["body_failed"]} body '
                                              f'failed, the silent object asked {asks} times')

    def rate(name: str, nbytes: int | None, seconds: float | None) -> None:
        if nbytes and seconds:
            notes.append(f'tensorfs {name}: {nbytes / seconds / 1e6:.0f} MB/s')

    if done('download-cold'):
        hub, vol = hub_bytes(rows['download-cold']['rec'])
        total = (hub or 0) + (vol or 0)
        # A datacenter's model cache serves a "cold" download as volume bytes: its times are its own baseline.
        src = 'hub' if hub and hub >= total / 2 else 'cache'
        if not total:
            fails.append('tensorfs download-cold: nothing was downloaded (the store held it already?)')
        rate('download-cold', total, rows['download-cold']['wall_s'])
        notes.append(f'tensorfs download-cold: {(hub or 0) / 2**30:.1f} GiB from the Hub, {(vol or 0) / 2**30:.1f} GiB from the volume')
        against(f'download-cold-{src}', rows['download-cold']['wall_s'], total, f'{total / 2**30:.1f} GiB ({src})')
    if done('download-again'):
        hub, _ = hub_bytes(rows['download-again']['rec'])
        if hub:
            fails.append(f'tensorfs download-again: {hub / 2**20:.0f} MiB came from the Hub again: the store did not hold it')
    def resumed(cut: str, resume: str) -> None:
        # Judged against the model, not against what the resume fetched: it must find what the cut run
        # landed (`local`) and fetch only the rest of the model's bytes. Bytes arrived count in `moved` as they arrive
        # (tfs-141), unfinished chunks too, and a chunk's kept bytes count in `local`, so this is every byte's fate.
        path = os.path.join(t, cut + '.cut')
        moved = int(open(path).read().strip() or 0) if os.path.exists(path) else 0
        rec = rows[resume]['rec']
        hub, vol = hub_bytes(rec)
        found = landed_bytes(rec)
        local = sum(b.get('local') or 0 for b in found) if found and all('local' in b for b in found) else None
        _, total = stage_progress(rows[resume]['evs'], 'downloading')
        fetched = (hub or 0) + (vol or 0)
        mib: Callable[[int | None], str] = lambda n: f'{(n or 0) / 2**20:.0f} MiB'
        if not moved:
            fails.append(f'tensorfs {resume}: the first download was never cut part-way: nothing was resumed')
        elif local is None or not total:
            fails.append(f'tensorfs {resume}: its record carries no held (`local`) bytes or no total: not judged')
        elif local < 0.8 * min(moved, total) or fetched > total - local + 64 * 2**20:
            fails.append(f'tensorfs {resume}: held {mib(local)} of {mib(total)} after {mib(moved)} had landed, '
                         f'then fetched {mib(fetched)}: it started over')
        elif vol or not hub:
            fails.append(f'tensorfs {resume}: {mib(hub)} from the Hub, {mib(vol)} from the volume: the private '
                         f'fixture never reaches a volume, so the Hub path was not exercised')
        else:
            notes.append(f'tensorfs {resume}: held {mib(local)} of {mib(total)} ({mib(moved)} had landed by the cut), '
                         f'fetched only the rest, {mib(hub)}, from the Hub')

    if done('download-cut', cancelled=True) and done('download-resume'):
        resumed('download-cut', 'download-resume')
    # The same run goes on once TensorD is back: its own record is the resume.
    if done('kill-cut'):
        resumed('kill-cut', 'kill-cut')
    up: float | None = None
    if done('upload'):
        evs = rows['upload']['evs']
        sec, (_, total) = stage_seconds(evs, 'uploading to'), stage_progress(evs, 'uploading to')
        up = (total / sec) if sec and total else None
        rate('upload', total, sec)
        against('upload', sec, total, f'{total / 2**20:.0f} MiB sent')
    if done('upload-again'):
        wall, first = rows['upload-again']['wall_s'], (rows.get('upload') or {}).get('wall_s') or 0
        if wall is None or wall > max(15, 0.1 * first):
            fails.append(f'tensorfs upload-again: {wall} s to send what the Hub holds (the first took {first:.0f} s): no dedup')
        else:
            notes.append(f'tensorfs upload-again: {wall:.1f} s for content the Hub held (the first took {first:.0f} s)')
    if done('upload-cut', cancelled=True) and done('upload-resume'):
        cut = os.path.join(t, 'upload-cut.cut')
        moved = int(open(cut).read()) if os.path.exists(cut) else 0
        evs = rows['upload-resume']['evs']
        sec, (_, total) = stage_seconds(evs, 'uploading to'), stage_progress(evs, 'uploading to')
        if not moved or not sec or not total or not up:
            fails.append(f'tensorfs upload-resume: cut at {moved} bytes, resumed in {sec} s: nothing to judge')
        else:
            limit = 1.5 * (total - 0.8 * moved) / up + 15
            (fails if sec > limit else notes).append(
                f'tensorfs upload-resume: {sec:.1f} s for the rest after {moved / 2**20:.0f} of {total / 2**20:.0f} MiB '
                f'(limit {limit:.1f} s at the upload\'s own rate)')
    if chosen('lora-alpha'):
        p = os.path.join(t, 'lora-alpha.json')
        cfg: LoraConfig
        try:
            (cfg,) = [c['config'] for c in json.load(open(p))['components'] if c.get('config', {}).get('peft_type') == 'LORA']
        except (OSError, ValueError, KeyError):
            fails.append(f'tensorfs lora-alpha: no LoRA component was imported (see {t}/lora-alpha/import.txt)')
        else:
            if cfg.get('lora_alpha') != cfg.get('r'):
                fails.append(f'tensorfs lora-alpha: committed lora_alpha {cfg.get("lora_alpha")} at r {cfg.get("r")}: kohya\'s '
                             'ss_network_alpha set the scale, which no loader of a file without .alpha tensors does')
            else:
                notes.append(f'tensorfs lora-alpha: lora_alpha {cfg["lora_alpha"]} = r, whatever the metadata says')
    # The lane's commit holds both components' weights encoded (inspect also held them to the source).
    if all(done(name) for name in ('mc-source', 'mc-upload', 'mc-quantize', 'mc-inspect')):
        inspected = rows['mc-inspect']['rec']
        assert inspected is not None
        held = cast(Inspected, inspected.get('result') or {}).get('tensors') or []  # cast: inspect's result is its Report
        encoded = sorted({h['component'] for h in held if h.get('scheme') == 'int8.convrot'})
        worst = max((cast(float, h['relative_error']) for h in held if h.get('relative_error') is not None), default=0)  # cast: the filter drops None
        ok = encoded == ['decoder', 'encoder']
        (notes if ok else fails).append(
            f'tensorfs multi-conversion: quantize run {rows["mc-quantize"]["run"]}, one call per component: '
            f'{", ".join(encoded) or "no component"} encoded{"" if ok else " (want decoder, encoder)"}; '
            f'worst relative error {worst:.4f}')
    # Refused at ingest, before the bulk downloads (tfs-137): the refusal's code, and what moved before it.
    for name, code, cap in (('family-refusal', 'FAMILY_MISMATCH', 64 << 20), ('layout-probe', 'LAYOUT_MISMATCH', 512 << 20)):
        if not chosen(name):
            continue
        r = rows.get(name)
        if r is None:
            fails.append(f'tensorfs {name}: did not run')
            continue
        rec = r['rec'] or {}
        why = json.dumps(rec.get('reason') or '')
        moved = max((stage_progress([e], 'download')[0] for e in r['evs']), default=0)
        if rec.get('status') != 'failed' or code not in why:
            fails.append(f'tensorfs {name}: run {r["run"]} {rec.get("status")}, not refused with {code}: {why[:300]}')
        elif moved > cap:
            fails.append(f'tensorfs {name}: {moved / 2**20:.0f} MiB downloaded before the refusal (at most {cap >> 20})')
        else:
            notes.append(f'tensorfs {name}: refused ({code}) after {moved / 2**20:.1f} MiB, {r["wall_s"]} s')
    if not fails:
        label = open(os.path.join(gate['out'], 'label')).read().strip()
        at = time.strftime('%Y-%m-%dT%H:%MZ', time.gmtime())
        for case, v in now.items():
            base.setdefault(case, []).append({**v, 'label': label, 'at': at})
    return fails, notes


if __name__ == '__main__' and sys.argv[1:2] == ['--past']:
    done_, total = stage_progress(events(sys.argv[2]), sys.argv[3])
    if total and done_ >= float(sys.argv[4]) * total:
        print(done_)
if __name__ == '__main__' and sys.argv[1:2] == ['--dropped']:
    print(dropped(events(sys.argv[2]), sys.argv[3]) or '')
