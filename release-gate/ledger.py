#!/usr/bin/env python3
"""The benchmark ledger (tracker design/benchmark-ledger.md, xs-211): one row per timed request, append-only in the private
tracker as `benchmarks/<sku>/<YYYY-MM>.jsonl`, so a new version is compared with what is recorded instead of re-running the old.

    ledger.py rows <dir> [--source gate|bench|session:<script>] [--sku SKU] [--packages DIR] [--cap CAP] [--flaw CELL=FLAW]   to stdout
    ledger.py append <rows.jsonl>            one commit to the tracker; a push race fetches the new tip and appends again
    ledger.py backfill <artifact id>         a run's ledger.jsonl that could not be pushed (no key, or the push kept racing)
    ledger.py query --sku SKU [--target PKG/FN]
    ledger.py compare <rows.jsonl> [--against runtime=0.46.0]   each row against the newest comparable recorded row
    ledger.py throttled <record.json>...     a line for each run whose GPU was held back (its times aren't its card's)
    ledger.py ask [--stop N] [--harness DIGEST] <target> <args>   a gate cell's request, its files by content (gate.sh `cell`)
    ledger.py skip <gate dir> <rows.jsonl> <cell> <target> <ask> [--packages DIR]   SKIP and the row it stands on, or why it runs
    ledger.py fetch <artifact id> <dir> [GLOB ...]             an artifact's small files, read by HTTP ranges (for seeding)

`<dir>` is a gate's or a session's output: every `<name>.json` run record (ab.sh's states.tsv gives its cells' state). Each
field has a reader: `compare` (workload, state, phases, host), `skip` (versions, request) or the report (memory, outcome)."""
from __future__ import annotations

import argparse, datetime, fcntl, fnmatch, hashlib, io, json, os, re, subprocess, sys, tempfile, time, urllib.error, urllib.request, zipfile
from collections.abc import Iterator
from typing import TypeAlias, cast

JSON: TypeAlias = str | int | float | bool | None | list['JSON'] | dict[str, 'JSON']

NOISE = 0.05  # Paul 11-15: within 5% is a tie
HOST_BOUND = ('start_s', 'construct_s', 'load_s')  # the execution's disk, host CPU and H2D-link phases
Row = dict[str, JSON]


def obj(v: JSON) -> dict[str, JSON]:
    return v if isinstance(v, dict) else {}


def arr(v: JSON) -> list[JSON]:
    return v if isinstance(v, list) else []


def num(v: JSON) -> float | None:
    return float(v) if isinstance(v, (int, float)) and not isinstance(v, bool) else None


def text(v: JSON) -> str | None:
    return v if isinstance(v, str) else None


def load(path: str) -> JSON:
    try:
        with open(path) as f:
            return cast(JSON, json.load(f))
    except (OSError, ValueError):
        return None


def measurements(rec: dict[str, JSON]) -> list[dict[str, JSON]]:
    """The run's own measurements, then each call's (H3's are in its calls)."""
    return [m for m in [obj(rec.get('measurements'))] + [obj(obj(c).get('measurements')) for c in arr(rec.get('calls'))] if m]


# A GPU busy LOADED_MS or more in a run and held back for heat, or at a power limit its host set, more than HOT of that: the
# machine's own "slowed down" (platform::gpu's LOADED_MS and HOT_SHARE, Calcifer's slow.rs). Not `throttled_share`: that counts
# every driver limit, and a healthy card at full load sits at its own rated power cap (every rtx-3090 host on record reads
# 0.45-0.99 there).
LOADED_MS, HOT = 20_000, 0.5


def throttled(rec: dict[str, JSON]) -> str:
    """Why this run's times aren't its card's: each loaded GPU that was held back, or '' when none was."""
    said: dict[int, str] = {}
    for m in measurements(rec):
        for d in (obj(x) for x in arr(obj(m.get('memory')).get('driver'))):
            heat, capped = (num(d.get(k)) or 0 for k in ('heat_share', 'capped_share'))
            if (num(d.get('busy_ms')) or 0) >= LOADED_MS and max(heat, capped) > HOT:
                gpu = int(num(d.get('gpu')) or 0)
                said[gpu] = (f"GPU {gpu} held back {max(heat, capped):.0%} of its run by {'heat' if heat > HOT else 'its host power limit'} "
                             f"({d.get('sm_mhz_mean')} of {d.get('sm_mhz_limit')} MHz, {d.get('temperature_max_c')} C)")
    return '; '.join(said[g] for g in sorted(said))


def phases(rec: dict[str, JSON]) -> dict[str, JSON]:
    """Seconds by phase from the record's report (its stages partition the execution: a `x / loading y` stage is carved out
    of x), plus the denoise steps and the LoRA bake. `compare` splits them into host-bound and GPU-bound."""
    p: dict[str, float] = {}
    loads: dict[str, float] = {}
    stages: dict[str, float] = {}
    for m in arr(obj(rec.get('report')).get('measured')):
        for s in (obj(x) for x in arr(obj(m).get('stages'))):
            name, sec = str(s.get('name') or ''), (num(s.get('ms')) or 0) / 1000
            if name.startswith('host · '):
                continue  # a host-side span inside the stages, not a stage of its own
            if ' / loading ' in name or name.startswith('loading '):
                part = name.rsplit('loading ', 1)[1]
                loads[part] = loads.get(part, 0) + sec
                k = 'load_s'
            else:
                k = {'executor start': 'start_s', 'construction': 'construct_s', 'unaccounted': 'other_s'}.get(name, 'compute_s')
                if k == 'compute_s':
                    stages[name] = stages.get(name, 0) + sec
            p[k] = p.get(k, 0) + sec
    steps: list[float] = []
    for m in measurements(rec):
        for k, v in obj(obj(m.get('attribution')).get('steps')).items():
            series = [x for x in (num(s) for s in arr(obj(obj(v).get('device')).get('series'))) if x is not None]
            if len(series) > len(steps) and not k.startswith(('decode', 'encode')):  # the denoise (or refine) loop
                steps = series
        for v in arr(obj(obj(m.get('memory')).get('stats')).get('varena')):
            bake = obj(obj(obj(obj(v).get('stats')).get('moved')).get('bake'))
            if ns := num(bake.get('device_ns')):
                p['bake_s'] = p.get('bake_s', 0) + ns / 1e9
    out: dict[str, JSON] = {k: round(v, 3) for k, v in p.items()}
    if loads:
        out['loads'] = {k: round(v, 3) for k, v in loads.items()}
    if stages:
        out['stages'] = {k: round(v, 3) for k, v in stages.items()}
    if steps:
        # A track's first entry is its warm-up mark (0 ms): step 1 is the first real step.
        real = steps[1:] if len(steps) > 1 and steps[0] <= 0 else steps
        out['steps'] = len(real)
        out['step1_s'] = round(real[0] / 1000, 3)
        if len(real) > 1:
            out['steady_s'] = round(sorted(real[1:])[len(real[1:]) // 2] / 1000, 3)
    if (e := num(rec.get('execution_ms'))) is not None:
        out['execution_s'] = round(e / 1000, 3)
    a, b = num(rec.get('created_at_ms')), num(rec.get('settled_at_ms'))
    if a and b:
        out['total_s'] = round((b - a) / 1000, 3)
    return out


def state(rec: dict[str, JSON]) -> dict[str, JSON]:
    """What the request found and left: LoRA bake blocks, derived bytes filled, persisted pages, the call memo."""
    stats = [obj(obj(v).get('stats')) for m in measurements(rec) for v in arr(obj(obj(m.get('memory')).get('stats')).get('varena'))]
    moved = [obj(s.get('moved')) for s in stats]
    return {'bake_blocks': int(sum(num(obj(m.get('bake')).get('blocks')) or 0 for m in moved)),
            'derived_bytes': int(sum(num(obj(m.get('derived')).get('bytes')) or 0 for m in moved)),
            'persisted': int(max([num(obj(s.get('baked_pages')).get('persisted')) or 0 for s in stats] or [0])),
            'memoized': obj(rec.get('memo')).get('memoized') is True or any(obj(c).get('memoized') is True for c in arr(rec.get('calls')))}


def host(rec: dict[str, JSON], machine: dict[str, JSON]) -> dict[str, JSON]:
    """The host fingerprint `compare` calibrates by: where it ran, the machine's measured link rates, the GPU's clocks and power."""
    rental = obj(machine.get('rental'))
    h: dict[str, JSON] = {'provider': rental.get('provider'), 'datacenter': rental.get('datacenter'),
                          'machine': text(machine.get('fingerprint')), 'gpu_name': rec.get('gpu')}
    gpus = arr(obj(machine.get('status')).get('gpus'))
    if gpus:
        h['driver'] = obj(gpus[0]).get('driver')
    for m in measurements(rec):
        for v in arr(obj(obj(m.get('memory')).get('stats')).get('varena')):
            ceiling = obj(obj(v).get('ceiling'))
            h['h2d_gbps'] = h.get('h2d_gbps') or num(ceiling.get('h2d_gbps'))
            h['disk_gbps'] = h.get('disk_gbps') or num(ceiling.get('disk_gbps'))
        for d in arr(obj(m.get('memory')).get('driver')):
            for k in ('sm_mhz_mean', 'sm_mhz_limit', 'power_max_w', 'throttled_share'):
                h[k] = h.get(k) or obj(d).get(k)
    return {k: v for k, v in h.items() if v is not None}


def versions(rec: dict[str, JSON], package: str, built: dict[str, JSON]) -> dict[str, JSON]:
    """The skip key's parts: the machine's software (and the candidate's wheel digests, which a dev build's version string
    does not name), the package's code, each model's release, manifest and lane, the attention table and the kernels picked."""
    v: dict[str, JSON] = {}
    for m in measurements(rec):
        for k in ('tensord', 'runtime', 'tensorfs'):
            v.setdefault(k, obj(m.get('software')).get(k))
    target = text(rec.get('target')) or ''
    v['package'] = target.split('@', 1)[1] if '@' in target else package
    v.update({k: x for k, x in built.items() if x})
    models: dict[str, JSON] = {}
    gemm: set[str] = set()
    attention: set[str] = set()
    for m in measurements(rec):
        for x in arr(m.get('models')):
            o = obj(x)
            models[str(o.get('slot'))] = {k: o.get(k) for k in ('repository', 'release', 'manifest', 'lane')}
        for o in (obj(x) for x in arr(m.get('observations'))):
            if o.get('name') == 'gemm.selected' and isinstance(o.get('value'), str):
                gemm.add(str(o['value']))
            if o.get('name') == 'attention.selected' and isinstance(o.get('value'), str):
                attention.add(str(o['value']))
    picked: dict[str, JSON] = {'gemm': list[JSON](sorted(gemm)), 'attention': list[JSON](sorted(attention))}
    v['models'], v['kernels'] = models, picked
    return v


def workload(rec: dict[str, JSON], cap: str | None) -> dict[str, JSON]:
    """What was asked: the function, the request without its seed (a digest), the seed, its shape, the GPUs and VRAM cap."""
    target = (text(rec.get('target')) or '').split('@', 1)[0]
    parts = [p for p in target.split('/') if p not in ('local', '_', '.')]
    req = obj(rec.get('request'))
    inp = dict(obj(req.get('input')))
    seed = inp.pop('seed', req.get('seed'))
    shape: dict[str, JSON] = {k: inp[k] for k in ('width', 'height', 'megapixels', 'aspect_ratio', 'steps', 'frames', 'turbo') if k in inp}
    seconds = sum(num(obj(b).get('duration')) or 0 for b in arr(inp.get('beats'))) or num(inp.get('duration'))
    if seconds:
        shape['seconds'] = seconds
    degree = max([num(obj(m.get('execution')).get('degree')) or 1 for m in measurements(rec)] or [1])

    def form(v: JSON) -> JSON:
        # The request with its words and boxes blanked: a gate draws its prompts and references per run, and the times of
        # two requests of one form compare (a word or enum without spaces, such as 16:9 or int8, is kept).
        if isinstance(v, dict):
            return {k: form(x) for k, x in v.items()}
        if isinstance(v, list):
            return [form(x) for x in v]
        return '·' if isinstance(v, str) and (' ' in v or '\n' in v or v.startswith('<')) else v
    return {'package': parts[-2] if len(parts) >= 2 else target, 'function': parts[-1] if parts else '', 'seed': seed,
            'request': 'sha256:' + hashlib.sha256(json.dumps(inp, sort_keys=True).encode()).hexdigest()[:32],
            'form': 'sha256:' + hashlib.sha256(json.dumps(form(inp), sort_keys=True).encode()).hexdigest()[:32],
            'shape': shape, 'gpus': int(degree), 'cap': cap}


def tree(packages: str | None, commit: str | None, package: str) -> str:
    """The package's code: its directory's git tree at the commit it ran from (`local@<commit>` without a checkout to read)."""
    if not commit:
        return 'local'
    # A session's PACKAGES holds one checkout per packages_ref, each with only its own commit.
    dirs = [packages] + sorted(os.path.join(packages, d) for d in os.listdir(packages)) if packages and os.path.isdir(packages) else []
    for d in dirs:
        r = subprocess.run(['git', '-C', d, 'rev-parse', '-q', '--verify', f'{commit}:{package}'], capture_output=True, text=True)
        if r.returncode == 0:
            return 'tree:' + r.stdout.strip()
    return f'local@{commit}'


def built(d: str) -> dict[str, JSON]:
    """A gate's versions known before its first cell: the candidate's digests (a dev build's version string does not name its
    build), its attention table, the cozy, Runtime and TensorFS the machine reported and the held model releases. Its rows
    carry them, and `skip` holds a cell to them."""
    cand = obj(load(os.path.join(d, 'candidate.json')))
    b: dict[str, JSON] = {f'{k}_sha256': cand.get(f'{k}_sha256') for k in ('cozy', 'runtime', 'tensorfs')}
    if os.path.exists(os.path.join(d, 'attention-table.json')):
        b['attention_table'] = 'sha256:' + hashlib.sha256(open(os.path.join(d, 'attention-table.json'), 'rb').read()).hexdigest()[:32]
    b['machine'] = obj(load(os.path.join(d, 'reported.json'))) or None
    b['held'] = arr(cand.get('models')) or None
    return b


def verdicts(d: str, names: list[str]) -> dict[str, JSON]:
    """Each cell's verdict from analyze.py's verdict.txt: FAIL when a FAIL line names it, first (the longest cell name the line
    starts with, as `h3-1080p-av:` does) or as a word of it (`pixels klein:`). A line that names no cell is the machine's or
    another check's, not a cell's. None before a verdict."""
    said = open(os.path.join(d, 'verdict.txt'), errors='replace').read().splitlines() if os.path.exists(os.path.join(d, 'verdict.txt')) else []
    if not any(l.startswith('VERDICT ') for l in said):
        return {}
    failed: set[str] = set()
    for line in (l[5:] for l in said if l.startswith('FAIL ')):
        first = [n for n in names if line.startswith(n) and line[len(n):len(n) + 1] in ('', ':', ' ', '-')]
        failed |= {max(first, key=len)} if first else {n for n in names if re.search(rf'(?:^|\s){re.escape(n)}(?=[:\s]|$)', line)}
    return {n: 'FAIL' if n in failed else 'PASS' for n in names}


def rows(d: str, source: str, run: str | None, packages: str | None, cap: str | None, flaws: dict[str, list[str]],
         sku: str | None = None) -> Iterator[Row]:
    """Each package run record in `d` (a session's or a gate's directory), as ledger rows. A cell's state is ab.sh's
    states.tsv when there is one; otherwise the rental was fresh and a function's first request is `cold`, the rest `warm`."""
    def file(name: str) -> str:
        return os.path.join(d, name)
    machines = [obj(load(file(n))) for n in ('show.json', 'machine-end.json') if os.path.exists(file(n))]
    machines = [obj(m.get('machine')) or m for m in machines]
    by_name = {text(m.get('name')): m for m in machines}
    cand = obj(load(file('candidate.json')))
    gate = built(d)
    temps: dict[str, list[str]] = {}
    if os.path.exists(file('states.tsv')):
        for line in open(file('states.tsv')).read().splitlines()[1:]:
            temps[line.split('\t')[0]] = line.split('\t')
    commits: dict[str, str] = {}
    caps: dict[str, str] = {}
    if os.path.exists(file('results.txt')):
        said = open(file('results.txt'), errors='replace').read()
        commits = {m[0]: m[1] for m in re.findall(r'^\S+ cell ([\w.-]+): ([0-9a-f]{7,40}) \./', said, re.M)}
        caps = {m[0]: m[1] for m in re.findall(r'^\S+ cell ([\w.-]+): caps VRAM at (\S+)', said, re.M)}
    def arm(name: str) -> str:
        # ab.sh's arm: states.tsv's, or before it (#3652) the cell name's `<arm>-<cell>`. Its cap is said at its first cell.
        return temps[name][1] if name in temps else name.rsplit('-', 1)[0] if caps else ''
    arm_caps = {arm(c): v for c, v in caps.items()}
    order: list[tuple[str, str]] = []
    if os.path.exists(file('cells.tsv')):
        order = [(f[0], f[1]) for f in (l.rstrip('\n').split('\t') for l in open(file('cells.tsv'))) if len(f) > 1]
    else:
        order = sorted(((n[:-5], '') for n in os.listdir(d) if n.endswith('.json')),
                       key=lambda c: num(obj(load(file(c[0] + '.json'))).get('created_at_ms')) or 0)
    asks = dict(l.rstrip('\n').split('\t')[:2] for l in open(file('asks.tsv')) if '\t' in l) if os.path.exists(file('asks.tsv')) else {}
    verdict = verdicts(d, [n for n, _ in order])
    seen: set[str] = set()
    for name, target in order:
        rec = obj(load(file(name + '.json')))
        if not rec or 'number' not in rec or target.startswith('/') or name.endswith(('-weights', '-weights-off')):
            continue  # not a run record, or a suite's own driver (kernels, gemm) rather than a package's function
        w = workload(rec, caps.get(name) or arm_caps.get(arm(name)) or cap)
        w['ask'] = asks.get(name)
        st = state(rec)
        fn = f"{arm(name)} {w['package']}/{w['function']}"
        t = temps.get(name)
        st['temperature'] = t[2] if t else ('warm' if fn in seen else 'cold')
        # What `cold` started from: ab.sh resets derived state and drops the page cache per arm (#3652; `none` before it); a
        # gate's rental starts with neither, but a function's weights were just downloaded, so they are in the page cache.
        st['reset'] = 'arm' if t else 'fresh-rental' if not caps else 'none'
        seen.add(fn)
        if t and len(t) > 8:
            st['fetched_weights'] = t[8] == 'true'
        ph = phases(rec)
        wt = obj(load(file(name + '-weights.json')))  # gate.sh `ahead`: a warm to `downloaded` before the function's first call
        if (a := num(wt.get('created_at_ms'))) and (b := num(wt.get('settled_at_ms'))):
            ph['download_s'] = round((b - a) / 1000, 3)
        peaks = [num(obj(x).get('peak_bytes')) or 0 for m in measurements(rec) for x in arr(obj(m.get('memory')).get('driver'))]
        procs = [num(obj(m.get('memory')).get('peak_allocated_bytes')) or 0 for m in measurements(rec)]
        machine = by_name.get(text(rec.get('machine'))) or (machines[0] if machines else {})
        at = num(rec.get('created_at_ms'))
        yield {
            'v': 1, 'id': f'{source}:{run}:{name}:{rec.get("number")}',
            'at': datetime.datetime.fromtimestamp(at / 1000, datetime.UTC).strftime('%Y-%m-%dT%H:%M:%SZ') if at else None,
            'source': {'kind': source, 'run': run, 'cell': name, 'record': rec.get('number')},
            'sku': sku or text(obj(machine.get('rental')).get('sku')) or re.sub(r'^(nvidia )?(geforce )?', '', str(rec.get('gpu') or '').lower()).replace(' ', '-'),
            'workload': w, 'versions': versions(rec, tree(packages, commits.get(name) or text(cand.get('packages_ref')), pkgdir(target) or str(w['package'])), gate),
            'state': st, 'phases': ph,
            'memory': {'peak_vram_gib': round(max(peaks or [0]) / 2**30, 2), 'peak_allocated_gib': round(max(procs or [0]) / 2**30, 2)},
            'host': host(rec, machine),
            'outcome': {'status': rec.get('status'), 'verdict': verdict.get(name),
                        'output': [obj(o).get('sha256') for o in arr(rec.get('output')) if obj(o).get('sha256')]},
            'flaws': list[JSON](flaws.get(name, []) + flaws.get('*', []) + (['host_throttled'] if throttled(rec) else []))}


# The tracker side: a shallow, sparse clone of `benchmarks/` in a cache of its own (never a shared checkout), one writer at a
# time. CI pushes with the TRACKER_LEDGER_KEY deploy key; a person's own machine with their git credentials.
def git(cwd: str, *args: str) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ, GIT_AUTHOR_NAME='Paul Fidika', GIT_AUTHOR_EMAIL='paul@fidika.com',
               GIT_COMMITTER_NAME='Paul Fidika', GIT_COMMITTER_EMAIL='paul@fidika.com', GIT_TERMINAL_PROMPT='0')
    if key := os.environ.get('TRACKER_LEDGER_KEY'):
        path = os.path.join(tempfile.gettempdir(), f'ledger-key-{os.getpid()}')
        if not os.path.exists(path):
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            os.write(fd, (key.strip() + '\n').encode())
            os.close(fd)
        env['GIT_SSH_COMMAND'] = f'ssh -i {path} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new'
    return subprocess.run(['git', '-C', cwd, *args], capture_output=True, text=True, env=env)


def repo_url(repo: str | None) -> str:
    return repo or os.environ.get('LEDGER_REPO') or ('git@github.com:cozy-creator/tracker.git' if os.environ.get('TRACKER_LEDGER_KEY')
                                                     else 'https://github.com/cozy-creator/tracker.git')


def checkout(repo: str) -> str:
    """The cache at the tracker's current master (it never holds a commit of its own between writes)."""
    cache = os.path.join(os.environ.get('XDG_CACHE_HOME') or os.path.expanduser('~/.cache'), 'cozy-ledger',
                         hashlib.sha256(repo.encode()).hexdigest()[:12])
    if not os.path.isdir(os.path.join(cache, '.git')):
        os.makedirs(cache, exist_ok=True)
        git(cache, 'init', '-q')
        git(cache, 'remote', 'add', 'origin', repo)
        git(cache, 'config', 'core.sparseCheckout', 'true')
        git(cache, 'sparse-checkout', 'set', 'benchmarks')
    if (r := git(cache, 'fetch', '-q', '--depth', '1', '--filter=blob:none', 'origin', 'master')).returncode:
        sys.exit(f'ledger: no fetch of {repo}: {r.stderr.strip()[:300]}')
    git(cache, 'reset', '-q', '--hard', 'FETCH_HEAD')
    return cache


def locked(repo: str) -> int:
    path = os.path.join(os.environ.get('XDG_CACHE_HOME') or os.path.expanduser('~/.cache'), 'cozy-ledger', 'lock')
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
    fcntl.flock(fd, fcntl.LOCK_EX)
    return fd


def append(path: str, repo: str, message: str | None = None) -> None:
    """The rows into `benchmarks/<sku>/<YYYY-MM>.jsonl`, one commit; a push race fetches the new tip and appends again."""
    new = [json.loads(l) for l in open(path) if l.strip()]
    if not new:
        return print('ledger: no rows')
    lock = locked(repo)
    for attempt in range(6):
        cache = checkout(repo)
        files: set[str] = set()
        for r in new:
            month = (text(r.get('at')) or datetime.datetime.now(datetime.UTC).strftime('%Y-%m'))[:7]
            rel = os.path.join('benchmarks', re.sub(r'[^a-z0-9.-]', '_', str(r.get('sku') or 'unknown').lower()), f'{month}.jsonl')
            os.makedirs(os.path.join(cache, os.path.dirname(rel)), exist_ok=True)
            have: set[JSON] = set()
            if os.path.exists(os.path.join(cache, rel)):
                have = {json.loads(l).get('id') for l in open(os.path.join(cache, rel)) if l.strip()}
            if r.get('id') in have:
                continue  # written once already: a retry, or the same run's rows sent again
            with open(os.path.join(cache, rel), 'a') as f:
                f.write(json.dumps(r, sort_keys=True, separators=(',', ':')) + '\n')
            files.add(rel)
        if not files:
            os.close(lock)
            return print('ledger: every row is there already')
        git(cache, 'add', '--', *sorted(files))
        src = obj(new[0].get('source'))
        git(cache, 'commit', '-q', '-m', message or f'ledger: {src.get("kind")} {src.get("run")} ({len(new)} rows)')
        if (r := git(cache, 'push', '-q', 'origin', 'HEAD:master')).returncode == 0:
            os.close(lock)
            return print(f'ledger: {len(new)} rows into {", ".join(sorted(files))}')
        print(f'ledger: push {attempt + 1}/6 refused: {r.stderr.strip()[:200]}', file=sys.stderr)
        time.sleep(min(60, 2 ** attempt))
    sys.exit('ledger: no push after 6 tries; the rows stay in ' + path)


def recorded(repo: str, sku: str) -> list[Row]:
    lock = locked(repo)
    cache = checkout(repo)
    d = os.path.join(cache, 'benchmarks', sku)
    out: list[Row] = []
    for f in sorted(os.listdir(d)) if os.path.isdir(d) else []:
        out += [json.loads(l) for l in open(os.path.join(d, f)) if l.strip()]
    os.close(lock)
    return out


# Comparing: a candidate row against the newest recorded row of the same SKU, function, request form, GPUs, cap and state.
UNFIT = {'cold_shared_state', 'local_stage_memo_miss', 'fetched_weights', 'no_ballast', 'host_throttled'}


def key(r: Row) -> tuple[str, ...]:
    """A gate cell's state is what its suite ran before it, so gate rows compare only with the same cell; a session's
    cells have ab.sh's declared state."""
    w = obj(r.get('workload'))
    st = obj(r.get('state'))
    src = obj(r.get('source'))
    cell = src.get('cell') if src.get('kind') == 'gate' else ''
    return tuple(str(x) for x in (r.get('sku'), w.get('package'), w.get('function'), w.get('form'), w.get('gpus'), w.get('cap'),
                                  st.get('temperature'), st.get('reset'), cell))


def classify(cand: Row, base: Row) -> str:
    """noise, host, regression or improvement, or undecided (naming the same-host A/B that decides it)."""
    pc, pb = obj(cand.get('phases')), obj(base.get('phases'))
    a, b = num(pb.get('execution_s')), num(pc.get('execution_s'))
    if not a or not b:
        return 'undecided: no execution time on one side'
    gap = b / a - 1
    if abs(gap) <= NOISE:
        return f'noise ({gap:+.1%})'
    kind = 'regression' if gap > 0 else 'improvement'
    hc, hb = obj(cand.get('host')), obj(base.get('host'))
    if hc.get('machine') and hc.get('machine') == hb.get('machine'):
        return f'{kind} ({gap:+.1%}, same machine)'
    if not hc or not hb:
        return f'undecided ({gap:+.1%}): no host fingerprint; run a same-host A/B (sessions/ab.sh, A B B A)'
    bound = sum((num(pc.get(k)) or 0) - (num(pb.get(k)) or 0) for k in HOST_BOUND)
    rest = (b - a) - bound
    links = {k: (num(hc.get(k)) or 0) / (num(hb.get(k)) or 1) for k in ('disk_gbps', 'h2d_gbps') if hb.get(k) and hc.get(k)}
    said = ', '.join(f'{k[:-5]} {x:.2f}x' for k, x in links.items())
    if abs(rest) <= NOISE * a:
        # Only host-bound phases moved: the host explains it when its links moved the same way (slower link, slower phase).
        if links and all((x < 1) == (bound > 0) for x in links.values()):
            return f'host ({gap:+.1%}: host-bound phases {bound:+.1f} s, links {said})'
        return f'undecided ({gap:+.1%}: host-bound phases {bound:+.1f} s, links {said or "unread"}); run a same-host A/B'
    if abs(bound) <= NOISE * a:
        # Only GPU-bound phases moved: a throttled side's SM clock explains what it scales away.
        clocks = [num(h.get('sm_mhz_mean')) for h in (hc, hb)]
        throttled = any((num(h.get('throttled_share')) or 0) > 0.3 for h in (hc, hb))
        if throttled and clocks[0] and clocks[1] and abs(b * clocks[0] / clocks[1] / a - 1) <= NOISE:
            return f'host ({gap:+.1%}: SM clock {clocks[0]:.0f} against {clocks[1]:.0f} MHz, throttled)'
        return f'{kind} ({gap:+.1%}: {rest:+.1f} s in GPU-bound phases)'
    return f'undecided ({gap:+.1%}: host-bound {bound:+.1f} s and GPU-bound {rest:+.1f} s both moved); run a same-host A/B'


def compare(path: str, repo: str, against: list[str]) -> None:
    cand = [json.loads(l) for l in open(path) if l.strip()]
    recorded_by: dict[str, list[Row]] = {}
    for c in cand:
        sku = str(c.get('sku'))
        if sku not in recorded_by:
            recorded_by[sku] = [r for r in recorded(repo, sku) if obj(r.get('outcome')).get('status') == 'completed'
                                and not obj(r.get('state')).get('memoized') and not set(map(str, arr(r.get('flaws')))) & UNFIT]
        st = obj(c.get('state'))
        if st.get('temperature') == 'warmup' or st.get('memoized'):
            continue
        mine = obj(c.get('versions'))
        base = [r for r in recorded_by[sku] if key(r) == key(c) and r.get('id') != c.get('id')
                and all(str(obj(r.get('versions')).get(k)) == v for k, v in (a.split('=', 1) for a in against))
                and {k: obj(r.get('versions')).get(k) for k in ('tensord', 'runtime', 'tensorfs', 'package')}
                != {k: mine.get(k) for k in ('tensord', 'runtime', 'tensorfs', 'package')}]
        w = obj(c.get('workload'))
        name = f"{sku} {w.get('package')}/{w.get('function')} {obj(c.get('source')).get('cell')} {st.get('temperature')}"
        if 'host_throttled' in arr(c.get('flaws')):
            print(f'{name}: void: its GPU was held back, so its times are no measure of this build; run it again on another host')
            continue
        if not base:
            print(f'{name}: no comparable row recorded')
            continue
        # The newest run's comparable rows, averaged: an A/B's two orders, or a gate's one row.
        run = obj(base[-1].get('source')).get('run')
        same = [r for r in base if obj(r.get('source')).get('run') == run]
        mean: Row = dict(same[-1])
        mean['phases'] = {k: sum(num(obj(r.get('phases')).get(k)) or 0 for r in same) / len(same) for k in HOST_BOUND + ('execution_s',)}
        print(f'{name}: {classify(c, mean)} against {", ".join(str(r.get("id")) for r in same)}')


# Skipping (design §4): a gate cell whose request and versions match a row of its last PASS on the SKU.
PATHS = re.compile(r'(?<![\w.:/])\.{0,2}/[^\s"\',;\]}]+')
SKIP_KEY = ('machine', 'cozy_sha256', 'runtime_sha256', 'tensorfs_sha256', 'attention_table', 'held', 'package')


def digest(path: str) -> str:
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for block in iter(lambda: f.read(1 << 20), b''):
            h.update(block)
    return 'sha256:' + h.hexdigest()


def ask(target: str, stop: str, harness: str, args: list[str]) -> str:
    """A cell's request before it runs: its function, its arguments with each file (a path, or a box mention gate.sh's `box`
    made, boxes.tsv) by its content, its STOP and its suite's or product's code. A pinned scenario and seed make it the same
    from gate to gate; any input that differs makes it differ."""
    boxes = dict(l.rstrip('\n').split('\t')[:2] for l in open('boxes.tsv') if '\t' in l) if os.path.exists('boxes.tsv') else {}
    words = [re.sub(r'<([\w.-]+)>', lambda m: f'<{m[1]}={boxes[m[1]]}>' if m[1] in boxes else m[0],
                    PATHS.sub(lambda m: digest(m[0]) if os.path.isfile(m[0]) else m[0], a)) for a in args]
    return 'sha256:' + hashlib.sha256(json.dumps([target, stop, harness, words]).encode()).hexdigest()[:32]


def pkgdir(target: str) -> str:
    """The --packages checkout's dir a gate cell's target `./<dir>/<function>` runs (gate.sh `pkg`), else ''."""
    return target.split('/')[1] if target.startswith('./') and target.count('/') == 2 else ''


def skip(d: str, path: str, name: str, target: str, asked: str, packages: str | None) -> bool:
    """SKIP and the row it stands on, or why the cell runs: the newest PASS row of this cell (its own SKU's rows, `path`)
    that asked the same and ran the same versions as this gate (`built`) and the same package code."""
    want = built(d)
    pkg = pkgdir(target)
    # Its committed code: a tracked file changed by hand is not (a lock a run writes, untracked, is no change).
    dirty = subprocess.run(['git', '-C', packages, 'status', '--porcelain', '--untracked-files=no', '--', pkg],
                           capture_output=True, text=True).stdout if pkg and packages else ''
    want['package'] = tree(packages, 'HEAD', pkg) if pkg and packages and not dirty else None
    if not str(want['package']).startswith('tree:'):
        print(f'{name}: runs: its code is not a committed package of the --packages checkout')
        return False
    passed = sorted((r for r in (json.loads(l) for l in open(path) if l.strip()) if obj(r.get('source')).get('cell') == name
                     and obj(r.get('outcome')).get('verdict') == 'PASS' and not arr(r.get('flaws'))), key=lambda r: str(r.get('at')))
    same = [r for r in passed if obj(r.get('workload')).get('ask') == asked]
    if not same:
        print(f'{name}: runs: ' + (f'its request differs from its last PASS, {passed[-1].get("id")}' if passed else 'no PASS of it is recorded'))
        return False

    def changed(r: Row) -> list[str]:
        v = obj(r.get('versions'))
        return [f'{k} {v.get(k) or "-"} -> {want.get(k) or "-"}' for k in SKIP_KEY if (v.get(k) or None) != (want.get(k) or None)]
    for r in reversed(same):
        if not changed(r):
            src = obj(r.get('source'))
            print(f'{name}: SKIP, unchanged since {r.get("id")} (run {src.get("run")}, record {src.get("record")}, {r.get("at")})')
            return True
    print(f'{name}: runs: since {same[-1].get("id")}, ' + '; '.join(changed(same[-1])))
    return False


# A GitHub Actions artifact's small files, read by HTTP ranges from its zip: a backfill or a seed needs records, not outputs.
class Ranged(io.RawIOBase):
    def __init__(self, url: str) -> None:
        self.url, self.pos = url, 0
        with urllib.request.urlopen(urllib.request.Request(url, method='HEAD')) as r:
            self.size = int(r.headers['Content-Length'])

    def seekable(self) -> bool:
        return True

    def readable(self) -> bool:
        return True

    def tell(self) -> int:
        return self.pos

    def seek(self, off: int, whence: int = 0) -> int:
        self.pos = off if whence == 0 else self.pos + off if whence == 1 else self.size + off
        return self.pos

    def readinto(self, b: bytearray | memoryview) -> int:  # type: ignore[override]
        if self.pos >= self.size:
            return 0
        end = min(self.pos + len(b), self.size) - 1
        with urllib.request.urlopen(urllib.request.Request(self.url, headers={'Range': f'bytes={self.pos}-{end}'})) as r:
            data = r.read()
        b[:len(data)] = data
        self.pos += len(data)
        return len(data)


def fetch(aid: str, out: str, globs: list[str]) -> None:
    token = subprocess.run(['gh', 'auth', 'token'], capture_output=True, text=True).stdout.strip()

    class Stop(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a: object, **k: object) -> None:
            return None
    try:
        urllib.request.build_opener(Stop).open(urllib.request.Request(
            f'https://api.github.com/repos/cozy-creator/cozy-creator/actions/artifacts/{aid}/zip', headers={'Authorization': f'Bearer {token}'}))
        sys.exit('ledger: the artifact gave no download location')
    except urllib.error.HTTPError as e:
        url = e.headers['Location']
    z = zipfile.ZipFile(io.BufferedReader(Ranged(url), buffer_size=1 << 20))
    os.makedirs(out, exist_ok=True)
    n = 0
    for name in z.namelist():
        top = os.path.dirname(name)
        if top and not (top.startswith('session-') and '/' not in top):
            continue  # a package checkout or the project's files, not the harness's own
        if any(fnmatch.fnmatch(os.path.basename(name), g) for g in globs) and z.getinfo(name).file_size < 64 << 20:
            with z.open(name) as src, open(os.path.join(out, os.path.basename(name)), 'wb') as f:
                f.write(src.read())
            n += 1
    print(f'ledger: {n} files into {out}', file=sys.stderr)


def main() -> None:
    ap = argparse.ArgumentParser(description=(__doc__ or '').split('\n')[0])
    sub = ap.add_subparsers(dest='cmd', required=True)
    r = sub.add_parser('rows')
    r.add_argument('dir')
    r.add_argument('--source', default='session', help='gate, bench or session (the harness script: session:ab)')
    r.add_argument('--run', default=os.environ.get('GITHUB_RUN_ID') or f'local-{int(time.time())}')
    r.add_argument('--packages', help="the packages checkout, for each package's code digest")
    r.add_argument('--cap', help='the VRAM cap every cell ran under, when the harness set one for the machine')
    r.add_argument('--sku', help="the harness's SKU, when the machine was not rented as one (gate.sh --on)")
    r.add_argument('--flaw', action='append', default=[], help='CELL=FLAW (CELL * for every row)')
    for name in ('append', 'backfill'):
        a = sub.add_parser(name)
        a.add_argument('rows' if name == 'append' else 'artifact')
        a.add_argument('--repo')
        a.add_argument('-m', '--message', help="the commit's message (a seed's sources)")
    q = sub.add_parser('query')
    q.add_argument('--sku', required=True)
    q.add_argument('--target', help='package/function')
    q.add_argument('--repo')
    c = sub.add_parser('compare')
    c.add_argument('rows')
    c.add_argument('--against', action='append', default=[], help='VERSION=VALUE the base rows must have (runtime=0.46.0)')
    c.add_argument('--repo')
    t = sub.add_parser('throttled')
    t.add_argument('records', nargs='+')
    a = sub.add_parser('ask')
    a.add_argument('target')
    a.add_argument('--stop', default='')
    a.add_argument('--harness', default='', help="the digest of the code that makes the cell (its suite's or product's)")
    a.add_argument('args', nargs=argparse.REMAINDER)
    k = sub.add_parser('skip')
    for n in ('gate', 'rows', 'cell', 'target', 'ask'):
        k.add_argument(n)
    k.add_argument('--packages')
    f = sub.add_parser('fetch')
    f.add_argument('artifact')
    f.add_argument('dir')
    f.add_argument('globs', nargs='*', default=['*.json', '*.tsv', 'results.txt', 'label'])
    args = ap.parse_args()
    if args.cmd == 'rows':
        flaws: dict[str, list[str]] = {}
        for spec in args.flaw:
            cell, _, flaw = spec.partition('=')
            flaws.setdefault(cell, []).append(flaw)
        for row in rows(args.dir, args.source, args.run, args.packages, args.cap, flaws, args.sku):
            print(json.dumps(row, sort_keys=True, separators=(',', ':')))
    elif args.cmd == 'append':
        append(args.rows, repo_url(args.repo), args.message)
    elif args.cmd == 'backfill':
        with tempfile.TemporaryDirectory() as d:
            fetch(args.artifact, d, ['ledger.jsonl'])
            append(os.path.join(d, 'ledger.jsonl'), repo_url(args.repo), args.message)
    elif args.cmd == 'query':
        for row in recorded(repo_url(args.repo), args.sku):
            w = obj(row.get('workload'))
            if not args.target or args.target == f"{w.get('package')}/{w.get('function')}":
                print(json.dumps(row, sort_keys=True))
    elif args.cmd == 'compare':
        compare(args.rows, repo_url(args.repo), args.against)
    elif args.cmd == 'throttled':
        for path in args.records:
            if why := throttled(obj(load(path))):
                print(f'{os.path.basename(path)[:-5]}: {why}')
    elif args.cmd == 'ask':
        print(ask(args.target, args.stop, args.harness, args.args))
    elif args.cmd == 'skip':
        sys.exit(0 if skip(args.gate, args.rows, args.cell, args.target, args.ask, args.packages) else 1)
    else:
        fetch(args.artifact, args.dir, args.globs)


if __name__ == '__main__':
    main()
