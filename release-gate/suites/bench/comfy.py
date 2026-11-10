"""Suite bench's ComfyUI rows: the requests and seeds of the cells that name a ComfyUI cell (cells.tsv `comfyui`), on the
same pod under the same host RAM, through the benchmarks runner's own setup and arm (cozy-creator/benchmarks, pinned in
comfyui.sh, which runs this).
    comfy.py jobs <gate dir>      here: one job per cell, its benchmarks cell, each run's prompt and seed, its cold record
    comfy.py run <jobs file>      on the pod, in the benchmarks checkout (/var/tmp/bm): rows into results/bench-comfy.jsonl
    comfy.py rows <gate dir>      here: comfyui/bench-comfy.jsonl as the group's ComfyUI rows (bench.py's Comfy)
A capped cell's job runs beside the suite's VRAM holder at the cell's level (pod.sh vram), the holder Cozy's run had."""
import argparse
import importlib
import json
import os
import statistics
import subprocess
import sys
from pathlib import Path
from types import ModuleType
from typing import NotRequired, TypedDict, cast

SESSION = 'bench-comfy'
POD_SH = '/var/tmp/bench-pod.sh'  # cells.sh copied it here


class Request(TypedDict):
    prompt: str
    seed: int


class Job(TypedDict):
    cell: str
    key: str  # the benchmarks cell (cells.toml)
    tiers: list[str]  # cold, w1, ...
    requests: list[Request]
    length: int | None  # a video's seconds
    cap_gib: int | None  # a capped cell's level: ComfyUI runs beside a holder that leaves this much of the GPU
    scout_s: float | None  # a matrix cell: Cozy's first warm total; ComfyUI stops after its own when faster (Paul 10-28: scout first)
    record: object  # the cell's cold Cozy record: the kernels and Sol schedule ComfyUI is matched to


class Tier(TypedDict):
    """One request: end to end, each node class's seconds, and the sampler's steps."""
    e2e_s: float | None
    stages: dict[str, float]
    steps_s: list[float]


class Row(TypedDict):
    """A cell's ComfyUI requests, as bench.py stores them beside the cell."""
    total_s: float | None  # the warm runs' median end-to-end, as Cozy's warm total
    cold_s: float | None
    steady_s: float | None  # the warm runs' median steady step (each run's median of steps 2..N)
    step1_s: float | None
    peak_mib: int | None  # the warm runs' highest NVML peak on the GPU, less the holder's
    held_mib: int  # the VRAM holder's, on a capped cell
    tiers: NotRequired[dict[str, Tier]]  # cold, w1, ...: where the time went
    status: str
    comfyui: str
    differences: list[str]
    source: NotRequired[str]


def text_of(o: object) -> str:
    """A prompt as the record carries it: a string, or 0.22's {text, references}."""
    if isinstance(o, dict):
        return str(o.get('text') or '')
    return str(o or '')


def request_of(record: dict[str, object]) -> tuple[Request, int | None]:
    req = record.get('request')
    inp = req.get('input') if isinstance(req, dict) else None
    inp = inp if isinstance(inp, dict) else {}
    seed = req.get('seed') if isinstance(req, dict) else None
    beats = inp.get('beats')
    if isinstance(beats, list) and beats:  # H3: its one beat's description is ComfyUI's prompt, its length the video's
        length = sum(int(b.get('duration') or 0) for b in beats if isinstance(b, dict))
        return {'prompt': text_of(beats[0].get('description') if isinstance(beats[0], dict) else ''), 'seed': int(str(seed))}, length
    return {'prompt': text_of(inp.get('prompt')), 'seed': int(str(seed))}, None


def total_s(record: dict[str, object]) -> float | None:
    a, b = record.get('created_at_ms'), record.get('settled_at_ms')
    return (b - a) / 1000 if isinstance(a, int) and isinstance(b, int) else None


def jobs(gate: str) -> list[Job]:
    sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..'))
    import bench  # release-gate/bench.py
    defs = {d['name']: d for d in bench.cells_file()}
    by: dict[str, list[tuple[str, str]]] = {}
    for line in Path(gate, 'bench.tsv').read_text().splitlines():
        cell, role, record = line.split('\t')[:3]
        if cell in defs and defs[cell]['comfyui']:
            by.setdefault(cell, []).append((role, record if os.path.isabs(record) else os.path.join(gate, record + '.json')))
    out: list[Job] = []
    matrix = '.matrix' in Path(gate, 'bench-group').read_text()
    for cell, runs in by.items():
        records = [cast(dict[str, object], bench.load_json(p)) for _, p in runs if os.path.exists(p)]  # the CLI's run records
        if len(records) != len(runs) or runs[0][0] != 'cold' or any(r.get('status') != 'completed' for r in records):
            continue  # Cozy's own runs did not all complete: nothing to match ComfyUI to
        reqs = [request_of(r) for r in records]
        out.append({'cell': cell, 'key': defs[cell]['comfyui'], 'tiers': ['cold'] + [f'w{i}' for i in range(1, len(runs))],
                    'requests': [q for q, _ in reqs], 'length': reqs[0][1], 'cap_gib': int(defs[cell]['cap']) if defs[cell]['cap'] else None,
                    'scout_s': total_s(records[1]) if matrix and len(records) > 1 else None, 'record': records[0]})
    return out


def holder(cap_gib: int | None) -> int:
    """GPU 0 as a card of `cap_gib` (pod.sh vram), or the whole card for None: the MiB the holder takes, -1 when it won't stand."""
    if cap_gib is None:
        subprocess.run(['sh', POD_SH, 'vram', 'off'], capture_output=True)
        return 0
    out = subprocess.run(['sh', POD_SH, 'vram', str(cap_gib << 30)], capture_output=True, text=True).stdout.split()
    return int(out[1]) >> 20 if len(out) >= 2 and out[1].isdigit() else -1


def run(path: str) -> None:
    """The benchmarks runner's setup, then its matched arm for each job; a run out of GPU memory at ComfyUI's own flags is run
    again with --lowvram, as its user would. A capped job runs beside the holder at its level."""
    here = os.path.dirname(os.path.abspath(path))
    sys.path.insert(0, here)
    bm: ModuleType = importlib.import_module('bench')  # cozy-creator/benchmarks: untyped, read through this boundary
    gpu: ModuleType = importlib.import_module('gpu')
    with open(path) as f:
        todo = cast(list[Job], json.load(f))  # comfy.py jobs wrote it
    keys = sorted({j['key'] for j in todo})
    s = bm.Session(argparse.Namespace(work='/var/tmp/bench', sku=None, session=SESSION))

    def do(cmd: str, env: dict[str, str] | None = None, check: bool = True) -> int:
        return int(bm.run(cmd, s, check=check, env=env))

    bm.checkout(s.work, keys, do)
    bm.environment(s.work, do)
    token = Path(here, '.hf-token')  # comfyui.sh: for gated files; the weights step alone sees it, ComfyUI never does
    if token.exists():
        os.environ['HF_TOKEN'] = token.read_text().strip()
        token.unlink()
    try:
        bm.weights(s.work, bm.comfy_files(keys, ['matched']), do)
    finally:
        os.environ.pop('HF_TOKEN', None)
    bm.Comfy(s, []).stop()  # a new venv's first start compiles every module, which is not the cold being measured
    s.say('setup done')
    sampler = gpu.Sampler()
    sampler.start()
    roots = {'cozy': gpu.tensorfs_roots(), 'comfy': [s.work / 'ComfyUI/models']}
    cap: int | None = None
    held = 0
    for j in todo:
        m = bm.CELLS['models'][j['key']]
        pin = bm.PINS['comfyui']
        if j['cap_gib'] != cap:
            cap, held = j['cap_gib'], holder(j['cap_gib'])
            s.say(f"a {cap} GiB card: holder {held} MiB" if cap else 'the whole card')
        base = {'v': 1, 'session': s.id, 'sku': s.sku, 'gpu': s.gpu, 'model': j['key'], 'name': m['name'], 'shape': m['shape'], 'cell': j['cell'],
                'comfyui': f"{pin['tag']} ({pin['commit'][:8]})", 'versions': {}, 'disk_settle': {'settled_mb_s': {'comfy': None}},
                'cap_gib': cap, 'held_mib': held}
        if held < 0:
            s.write(base | {'engine': 'comfy', 'arm': 'matched', 'tier': 'cold', 'status': 'no holder', 'error': f'no {cap} GiB holder', 'e2e_s': None,
                            'phases': {}, 'meters': {}})
            continue
        scout = j.get('scout_s')

        def go_on(done: list[dict[str, object]], scout: float | None = scout) -> bool:
            """A scout's timed request (w1) faster than Cozy's ends the cell there: a loss goes to a fix, not to more requests."""
            last = done[-1]
            e2e = last.get('e2e_s')
            return not (scout is not None and last.get('tier') == 'w1' and isinstance(e2e, (int, float)) and e2e < scout)
        rows = bm.comfy_arm(s, m, j['key'], 'matched', j['tiers'], j['requests'], j['length'], bm.served(j['record']), sampler, roots, base, go_on)
        if rows and rows[-1]['status'] == 'oom':
            s.say(f"{j['cell']}: out of GPU memory at ComfyUI's own flags: again with --lowvram")
            match = bm.match

            def lowvram(*a: object) -> object:
                mt = match(*a)
                return mt | {'flags': mt['flags'] + ['--lowvram']}
            setattr(bm, 'match', lowvram)
            bm.comfy_arm(s, m, j['key'], 'matched', j['tiers'], j['requests'], j['length'], bm.served(j['record']), sampler, roots, base, go_on)
            setattr(bm, 'match', match)
    holder(None)
    s.say('done')


def rows(gate: str) -> dict[str, Row]:
    """Each cell's ComfyUI row: a --lowvram rerun replaces the failed one it followed."""
    path = Path(gate, 'comfyui', f'{SESSION}.jsonl')
    by: dict[str, dict[str, dict[str, object]]] = {}
    for line in path.read_text().splitlines() if path.exists() else []:
        d = cast(dict[str, object], json.loads(line))  # the benchmarks runner's row
        by.setdefault(str(d.get('cell')), {})[str(d.get('tier'))] = d
    out: dict[str, Row] = {}
    def num(d: dict[str, object], *path: str) -> float | None:
        o: object = d
        for k in path:
            o = o.get(k) if isinstance(o, dict) else None
        return float(o) if isinstance(o, (int, float)) else None

    for cell, tiers in by.items():
        ok = {t: d for t, d in tiers.items() if d.get('status') == 'ok' and d.get('e2e_s') is not None}
        warm = [d for t, d in ok.items() if t != 'cold']
        held = int(num(next(iter(tiers.values())), 'held_mib') or 0)

        def mid(*path: str, rows: list[dict[str, object]] = warm) -> float | None:
            vals = [v for d in rows if (v := num(d, *path)) is not None]
            return round(statistics.median(vals), 4) if vals else None
        peaks = [v for d in warm if (v := num(d, 'meters', 'peak_vram_mib')) is not None]
        failed = next((d for d in tiers.values() if d.get('status') != 'ok'), None)
        config = tiers.get('cold', {}).get('config')

        def tier(d: dict[str, object]) -> Tier:
            ph = d.get('phases')
            ph = ph if isinstance(ph, dict) else {}
            st, steps = ph.get('stages'), ph.get('steps_s')
            return {'e2e_s': num(d, 'e2e_s'), 'stages': {str(k): float(v) for k, v in st.items() if isinstance(v, (int, float))} if isinstance(st, dict) else {},
                    'steps_s': [float(x) for x in steps if isinstance(x, (int, float))] if isinstance(steps, list) else []}
        out[cell] = {'total_s': round(v, 2) if (v := mid('e2e_s')) is not None else None, 'cold_s': num(ok['cold'], 'e2e_s') if 'cold' in ok else None,
                     'steady_s': mid('phases', 'step'), 'step1_s': mid('phases', 'step1'),
                     'peak_mib': int(max(peaks)) - held if peaks else None, 'held_mib': held, 'tiers': {t: tier(d) for t, d in tiers.items()},
                     'status': 'ok' if not failed else f"{failed.get('status')}: {str(failed.get('error'))[:160]}",
                     'comfyui': str(next((d.get('comfyui') for d in tiers.values()), '')),
                     'differences': [str(x) for x in (config.get('differences') or [])] if isinstance(config, dict) else [],
                     'source': str(path)}
    return out


if __name__ == '__main__':
    verb, arg = sys.argv[1], sys.argv[2]
    if verb == 'jobs':
        print(json.dumps(jobs(arg)))
    elif verb == 'run':
        run(arg)
    else:
        print(json.dumps(rows(arg), indent=1))
