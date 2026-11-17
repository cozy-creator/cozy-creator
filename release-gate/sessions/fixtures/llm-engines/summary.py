"""llm-engines.sh's summary: each phase of the cozy path (runs.jsonl, runs/<label>.json, phases.txt, smi.csv) and of the engine
(<name>.summary.json, .rows.json, .load.json), and each photo's answers side by side (answers.json).

  python3 summary.py <dir>
"""
import difflib
import json
import os
import statistics
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', '..'))
from runs import Run  # noqa: E402  release-gate/runs.py


def phases(d: str) -> dict[str, tuple[float, float]]:
    marks: dict[str, float] = {}
    try:
        for line in open(os.path.join(d, 'phases.txt')):
            t, name = line.split()
            marks[name] = float(t)
    except FileNotFoundError:
        pass
    out = {}
    for name, t in marks.items():
        if name.endswith('-start'):
            base = name[: -len('-start')]
            end = marks.get(f'{base}-end')
            if end:
                out[base] = (t, end)
    return out


def peak(d: str, window: tuple[float, float]) -> int | None:
    best = None
    try:
        for line in open(os.path.join(d, 'smi.csv')):
            parts = [p.strip() for p in line.split(',')]
            if len(parts) >= 2 and parts[0].isdigit() and window[0] <= int(parts[0]) <= window[1] + 1:
                best = max(best or 0, int(parts[1]))
    except FileNotFoundError:
        pass
    return best


def main() -> int:
    d = sys.argv[1]
    rows = [json.loads(line) for line in open(os.path.join(d, 'runs.jsonl'))] if os.path.exists(os.path.join(d, 'runs.jsonl')) else []
    marks = phases(d)
    texts: dict[str, dict[str, str]] = {}
    fails = []
    by_phase: dict[str, list[dict]] = {}
    for r in rows:
        run = Run.read(os.path.join(d, 'runs', f"{r['label']}.json"))
        phase, _, item = r['label'].partition('-')
        r['status'] = run.status if run else 'no record'
        r['exec_s'] = run.execution_ms / 1000 if run and run.execution_ms else None
        result = run.result if run and isinstance(run.result, dict) else {}
        said = result.get('texts') or ([result['text']] if isinstance(result.get('text'), str) else [])
        r['texts'] = said
        if run:
            r['lane'] = ', '.join(f"{m.get('repository')} · {m.get('lane')}" for m in run.models)
            r['kernels'] = f"{', '.join(run.attention) or '-'} · {', '.join(k for _, k in run.gemm) or '-'}"
            r['vram'] = run.vram
        if r['status'] != 'completed':
            fails.append(f"{r['label']} (run {r.get('run')}): {r['status']}")
        by_phase.setdefault(phase, []).append(r)
        if phase in ('c1', 'videos') and said:
            texts.setdefault(item, {})['cozy'] = said[0]
        if phase == 'b16':
            for k, t in enumerate(said):
                texts.setdefault(f'b16-{int(item) + k}', {})['cozy-b16'] = t
    print('## Cozy path\n')
    print('| phase | runs | wall s | per caption s (wall) | execution s (mean) | peak VRAM MiB | lane | kernels |')
    print('|---|---|---|---|---|---|---|---|')
    for phase, rs in by_phase.items():
        w = marks.get(phase if phase != 'cold' else 'cold')
        wall = round(w[1] - w[0], 1) if w else round(sum(r['wall_s'] for r in rs), 1)
        n = sum(max(1, len(r['texts'])) for r in rs)
        ex = [r['exec_s'] for r in rs if r['exec_s']]
        print(f"| {phase} | {len(rs)} | {wall} | {wall / max(n, 1):.2f} | {statistics.mean(ex) if ex else 0:.2f} |"
              f" {peak(d, w) if w else '?'} | {rs[0].get('lane', '?')} | {rs[0].get('kernels', '?')} |")
    for f in sorted(os.listdir(d)):
        if not f.endswith('.summary.json'):
            continue
        name = f[: -len('.summary.json')]
        print(f'\n## Engine {name}\n')
        try:
            for line in open(os.path.join(d, f'{name}.load.json')):
                print(f'- {line.strip()}')
        except FileNotFoundError:
            pass
        print('\n| phase | n | errors | wall s | out tok/s | TTFT mean s | latency p50 s | decode tok/s (per stream) | prompt tok |')
        print('|---|---|---|---|---|---|---|---|---|')
        for s in json.load(open(os.path.join(d, f))):
            print(f"| {s['phase']} | {s['n']} | {s['errors']} | {s['wall_s']} | {s['out_tok_s']} | {s.get('ttft_mean_s')} |"
                  f" {s.get('lat_p50_s')} | {s.get('decode_tps_mean')} | {s.get('prompt_tok_mean')} |")
        er = json.load(open(os.path.join(d, f'{name}.rows.json')))
        for r in er.get('images_c1', []) + er.get('videos_c1', []):
            texts.setdefault(r.get('item', f"#{r['id']}"), {})[name] = r.get('text', '')
    pairs = [(v['cozy'], v[e]) for v in texts.values() if 'cozy' in v for e in v if e not in ('cozy', 'cozy-b16')]
    if pairs:
        same = sum(a.strip() == b.strip() for a, b in pairs)
        ratio = statistics.mean(difflib.SequenceMatcher(None, a.strip(), b.strip()).ratio() for a, b in pairs)
        print(f'\nParity, engine vs cozy one-at-a-time: {same}/{len(pairs)} identical, mean similarity {ratio:.3f}')
    json.dump(texts, open(os.path.join(d, 'answers.json'), 'w'), indent=1)
    for fail in fails:
        print(f'FAIL {fail}')
    return 1 if fails else 0


if __name__ == '__main__':
    sys.exit(main())
