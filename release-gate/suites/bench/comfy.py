"""Suite bench's ComfyUI rows: the requests and seeds of the cells that name a ComfyUI cell (cells.tsv `comfyui`), on the
same pod under the same host RAM, through the benchmarks runner's own setup and arm (cozy-creator/benchmarks, pinned in
comfyui.sh, which runs this).
    comfy.py jobs <gate dir>      here: one job per cell, its benchmarks cell, each run's prompt and seed, its cold record
    comfy.py run <jobs file>      on the pod, in the benchmarks checkout (/var/tmp/bm): rows into results/bench-comfy.jsonl
    comfy.py rows <gate dir>      here: comfyui/bench-comfy.jsonl as the group's ComfyUI rows (bench.py's Comfy)"""
import argparse
import importlib
import json
import os
import statistics
import sys
from pathlib import Path
from types import ModuleType
from typing import NotRequired, TypedDict, cast

SESSION = 'bench-comfy'


class Request(TypedDict):
    prompt: str
    seed: int


class Job(TypedDict):
    cell: str
    key: str  # the benchmarks cell (cells.toml)
    tiers: list[str]  # cold, w1, ...
    requests: list[Request]
    length: int | None  # a video's seconds
    record: object  # the cell's cold Cozy record: the kernels and Sol schedule ComfyUI is matched to


class Row(TypedDict):
    """One ComfyUI call, as bench.py stores it beside the cell."""
    total_s: float | None  # the warm runs' median end-to-end, as Cozy's warm total
    cold_s: float | None
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
    for cell, runs in by.items():
        records = [cast(dict[str, object], bench.load_json(p)) for _, p in runs if os.path.exists(p)]  # the CLI's run records
        if len(records) != len(runs) or runs[0][0] != 'cold' or any(r.get('status') != 'completed' for r in records):
            continue  # Cozy's own runs did not all complete: nothing to match ComfyUI to
        reqs = [request_of(r) for r in records]
        out.append({'cell': cell, 'key': defs[cell]['comfyui'], 'tiers': ['cold'] + [f'w{i}' for i in range(1, len(runs))],
                    'requests': [q for q, _ in reqs], 'length': reqs[0][1], 'record': records[0]})
    return out


def run(path: str) -> None:
    """The benchmarks runner's setup, then its matched arm for each job; a run out of GPU memory at ComfyUI's own flags is run
    again with --lowvram, as its user would."""
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
    bm.weights(s.work, bm.comfy_files(keys, ['matched']), do)
    bm.Comfy(s, []).stop()  # a new venv's first start compiles every module, which is not the cold being measured
    s.say('setup done')
    sampler = gpu.Sampler()
    sampler.start()
    roots = {'cozy': gpu.tensorfs_roots(), 'comfy': [s.work / 'ComfyUI/models']}
    for j in todo:
        m = bm.CELLS['models'][j['key']]
        pin = bm.PINS['comfyui']
        base = {'v': 1, 'session': s.id, 'sku': s.sku, 'gpu': s.gpu, 'model': j['key'], 'name': m['name'], 'shape': m['shape'], 'cell': j['cell'],
                'comfyui': f"{pin['tag']} ({pin['commit'][:8]})", 'versions': {}, 'disk_settle': {'settled_mb_s': {'comfy': None}}}
        rows = bm.comfy_arm(s, m, j['key'], 'matched', j['tiers'], j['requests'], j['length'], bm.served(j['record']), sampler, roots, base)
        if rows and rows[-1]['status'] == 'oom':
            s.say(f"{j['cell']}: out of GPU memory at ComfyUI's own flags: again with --lowvram")
            match = bm.match

            def lowvram(*a: object) -> object:
                mt = match(*a)
                return mt | {'flags': mt['flags'] + ['--lowvram']}
            setattr(bm, 'match', lowvram)
            bm.comfy_arm(s, m, j['key'], 'matched', j['tiers'], j['requests'], j['length'], bm.served(j['record']), sampler, roots, base)
            setattr(bm, 'match', match)
    s.say('done')


def rows(gate: str) -> dict[str, Row]:
    """Each cell's ComfyUI row: a --lowvram rerun replaces the failed one it followed."""
    path = Path(gate, 'comfyui', f'{SESSION}.jsonl')
    by: dict[str, dict[str, dict[str, object]]] = {}
    for line in path.read_text().splitlines() if path.exists() else []:
        d = cast(dict[str, object], json.loads(line))  # the benchmarks runner's row
        by.setdefault(str(d.get('cell')), {})[str(d.get('tier'))] = d
    out: dict[str, Row] = {}
    for cell, tiers in by.items():
        ok = {t: float(str(d['e2e_s'])) for t, d in tiers.items() if d.get('status') == 'ok' and d.get('e2e_s') is not None}
        warm = [v for t, v in ok.items() if t != 'cold']
        failed = next((d for d in tiers.values() if d.get('status') != 'ok'), None)
        config = tiers.get('cold', {}).get('config')
        out[cell] = {'total_s': round(statistics.median(warm), 2) if warm else None, 'cold_s': ok.get('cold'),
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
