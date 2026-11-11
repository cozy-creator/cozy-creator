#!/usr/bin/env python3
"""Paul's sign-off for xs-171's bars, from the calibration round's gate directories (README.md, Calibration).

    signoff.py OUT GATE_DIR...      one gate directory per class (its artifact, with calibrate.sh's signoff/)

OUT gets each class's outputs (`<class>/<cell>-<arm>.png|mp4`) and manifest.json: the proposed bars, every kernel's
verdict per class, and one row per output {class, model, kernel, level, seed, metrics, pass}. The bars:
- micro: a kernel's worst-head relative L2 against FP32 over the quality cells' sites, at most 0.01 at 16 bits and 0.04
  at 8 (measurement-design §3.2's starting bars);
- end to end: S_8, the lowest layout SSIM against 16 bits over today's choice's pictures (the table arm), rounded down to
  0.005, and E_8, the largest worst eighth over its videos, rounded up to 0.005 (§3.4: no worse than what has shipped).
A pinned kernel passes when every output of it is within S_8/E_8 and its micro error is within its level's bar."""

import json
import math
import shutil
import sys
from pathlib import Path
from typing import TypedDict, cast

MICRO = {16: 0.01, 8: 0.04}


class Pair(TypedDict, total=False):
    kind: str
    ssim: float
    eighths: list[float]


class Row(TypedDict):
    """One output (calibrate.sh's signoff.jsonl)."""
    class_: str
    card: str
    cell: str
    model: str
    arm: str
    seed: int
    run: str
    status: str
    reason: str
    file: str
    pair: Pair | None


class Micro(TypedDict):
    level: int
    worst: float | None
    site: str
    corpus: str
    finite: bool


def detail(path: Path) -> dict[str, object]:
    try:
        found: dict[str, object] = json.loads(json.loads(path.read_text())['result']['detail'])
        return found
    except (OSError, KeyError, TypeError, ValueError):
        return {}


def micro(gate: Path) -> dict[str, Micro]:
    """Each kernel's worst head over its quality cell's sites."""
    found: dict[str, Micro] = {}
    for path in sorted(gate.glob('attention-quality-*.json')):
        d = detail(path)
        errors = cast(list[dict[str, object]], d.get('errors') or [])
        worst = max(errors, key=lambda e: cast(float, e.get('worst') or 0.0), default=None)
        found[str(d.get('kernel'))] = {'level': cast(int, d.get('level') or 0), 'worst': cast(float, worst['worst']) if worst else None,
                                       'site': str(worst.get('site', '')) if worst else '', 'corpus': str(d.get('corpus', '')),
                                       'finite': all(bool(e.get('finite')) for e in errors)}
    return found


def distance(pair: Pair | None) -> tuple[str, float] | None:
    if not pair:
        return None
    if 'ssim' in pair:
        return 'ssim', pair['ssim']
    if pair.get('eighths'):
        return 'worst_eighth', max(pair['eighths'])
    return None


def main(argv: list[str]) -> int:
    if len(argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    out = Path(argv[1])
    out.mkdir(parents=True, exist_ok=True)
    rows: list[tuple[Path, Row]] = []
    micros: dict[str, dict[str, Micro]] = {}
    for gate in map(Path, argv[2:]):
        klass = str(detail(gate / 'attention-inventory.json').get('klass', ''))
        micros[klass] = micro(gate)
        path = gate / 'signoff' / 'signoff.jsonl'
        for line in path.read_text().splitlines() if path.exists() else []:
            raw = json.loads(line)
            raw['class_'] = raw.pop('class')
            rows.append((gate, cast(Row, raw)))
    table: list[tuple[str, float]] = []
    for _, r in rows:
        d = distance(r['pair']) if r['arm'] == 'table' else None
        if d:
            table.append(d)
    pictures, videos = [v for k, v in table if k == 'ssim'], [v for k, v in table if k == 'worst_eighth']
    s8 = math.floor(min(pictures) / 0.005) * 0.005 if pictures else None
    e8 = math.ceil(max(videos) / 0.005) * 0.005 if videos else None
    outputs: list[dict[str, object]] = []
    e2e_ok: dict[tuple[str, str], bool] = {}
    for gate, r in rows:
        klass, arm, d = r['class_'], r['arm'], distance(r['pair'])
        m = micros.get(klass, {}).get(arm)
        e2e = None if arm == 'exact' else (d is not None and ((d[0] == 'ssim' and s8 is not None and d[1] >= s8)
                                                              or (d[0] == 'worst_eighth' and e8 is not None and d[1] <= e8)))
        if e2e is not None and arm != 'table':
            e2e_ok[(klass, arm)] = e2e_ok.get((klass, arm), True) and e2e
        file = ''
        if r['file'] and (gate / r['file']).exists():
            file = f"{klass.replace('/', '_')}/{Path(r['file']).name}"
            (out / file).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(gate / r['file'], out / file)
        outputs.append({'file': file, 'class': klass, 'card': r['card'], 'model': r['model'], 'cell': r['cell'],
                        'kernel': {'exact': '16-bit reference', 'table': "today's choice (no pin)"}.get(arm, arm),
                        'level': 16 if arm == 'exact' else (m['level'] if m else 8), 'seed': r['seed'], 'run': r['run'],
                        'status': r['status'], 'reason': r['reason'],
                        'metrics': {'end_to_end': dict([d]) if d else None, 'micro': m}, 'pass': e2e})
    kernels: dict[str, dict[str, dict[str, object]]] = {}
    for klass, found in micros.items():
        for kernel, m in found.items():
            bar, worst = MICRO.get(m['level']), m['worst']
            micro_ok = None if bar is None or worst is None else m['finite'] and worst <= bar
            e2e = e2e_ok.get((klass, kernel))
            kernels.setdefault(klass, {})[kernel] = {'level': m['level'], 'micro': m, 'micro_bar': bar, 'micro_pass': micro_ok,
                                                     'end_to_end_pass': e2e,
                                                     'pass': micro_ok if e2e is None else bool(micro_ok) and e2e}
    manifest = {'bars': {'micro_worst_head_rel_l2': {str(k): v for k, v in MICRO.items()},
                         'S_8_min_layout_ssim': s8, 'E_8_max_worst_eighth': e8,
                         'how': "S_8/E_8: today's choice's worst distance from 16 bits, rounded outward to 0.005; micro: "
                                "measurement-design §3.2's starting bars, on the quality cells' corpus (see each kernel's)"},
                'kernels': kernels, 'outputs': outputs}
    (out / 'manifest.json').write_text(json.dumps(manifest, indent=1, sort_keys=True) + '\n')
    print(f"{len(outputs)} outputs, {sum(len(k) for k in kernels.values())} kernels in {len(kernels)} classes; S_8 {s8}, E_8 {e8}")
    return 0


if __name__ == '__main__':
    raise SystemExit(main(sys.argv))
