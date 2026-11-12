#!/usr/bin/env python3
"""Paul's sign-off for xs-171's bars, from the calibration round's gate directories (README.md, Calibration).

    signoff.py OUT [--corpus DIR] GATE_DIR...   one gate directory per class (its artifact, with calibrate.sh's signoff/);
                                               DIR: the attention-corpus gate's corpus/, for the operands' outliers

OUT gets each class's outputs (`<class>/<cell>-<arm>.png|mp4`) and manifest.json: the proposed bars, every kernel's
verdict per class, and one row per output {class, model, kernel, level, seed, metrics, pass}. The bars:
- micro: a kernel's worst-head relative L2 against FP32 over the quality cells' captured sites (the corpus; the synthetic
  sites' worst is shown beside it), at most 0.01 at 16 bits and 0.04 at 8 (measurement-design §3.2's starting bars);
- end to end: S_8, the lowest layout SSIM against 16 bits over today's choice's pictures (the table arm), rounded down to
  0.005, and E_8, the largest worst eighth over its videos, rounded up to 0.005 (§3.4: no worse than what has shipped).
A pinned kernel passes when every output of it is within S_8/E_8 and its micro error is within its level's bar.
Each picture or video pair is also labelled in plain words (LOOKS). real-data.md lists every captured site where an
8-bit kernel misses its bar: the error, whether the Runtime holds the site to 16 bits (`exact`: an exact path or the
outlier-key rule), which operand's heads carry the outlier (max/RMS of Q, K and V), and the model's pictures beside it."""

import json
import math
import shutil
import sys
from pathlib import Path
from typing import TypedDict, cast

MICRO = {16: 0.01, 8: 0.04}
# What a distance from the 16-bit reference looks like, against what is on record (measurement-design §3.3): the same
# request and seed across builds and pods scores SSIM 0.99-1.00, another seed 0.35-0.70; same-seed H3 across kernel paths
# 0.017-0.084 in its worst eighth, another seed 0.23-0.24.
LOOKS = {'ssim': [(0.98, 'the same picture'), (0.93, 'the same picture, details changed'), (0.85, 'a changed picture'),
                  (0.0, 'a different picture (toward the another-seed range, 0.35-0.70)')],
         'worst_eighth': [(0.05, 'the same video'), (0.085, 'the same video, details changed'), (0.15, 'a changed video'),
                          (math.inf, 'a different video (the another-seed range, 0.23-0.24)')]}
# The corpus's model names (attention-corpus cells) and the packages the arms run.
PACKAGE = {'h3-15s': 'minimax-h3', 'qwen4mp': 'qwen-image-2', 'moss': 'moss-image', 'klein': 'flux-3-klein'}


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
    """A kernel's worst head: on the captured corpus (`kind: "captured"` sites) where it has one, which the bar judges, and
    on the synthetic sites beside it."""
    level: int
    worst: float | None
    basis: str  # captured, else synthetic
    captured_worst: float | None
    captured_site: str
    synthetic_worst: float | None
    synthetic_site: str
    finite: bool


def detail(path: Path) -> dict[str, object]:
    try:
        found: dict[str, object] = json.loads(json.loads(path.read_text())['result']['detail'])
        return found
    except (OSError, KeyError, TypeError, ValueError):
        return {}


def worst(errors: list[dict[str, object]]) -> tuple[float | None, str]:
    top = max(errors, key=lambda e: cast(float, e.get('worst') or 0.0), default=None)
    return (cast(float, top['worst']), str(top.get('site', ''))) if top else (None, '')


def micro(gate: Path) -> dict[str, Micro]:
    """Each kernel's worst head over its quality cell's sites, captured and synthetic apart."""
    found: dict[str, Micro] = {}
    for path in sorted(gate.glob('attention-quality-*.json')):
        d = detail(path)
        errors = cast(list[dict[str, object]], d.get('errors') or [])
        (cw, cs), (sw, ss) = worst([e for e in errors if e.get('kind') == 'captured']), worst([e for e in errors if e.get('kind') != 'captured'])
        found[str(d.get('kernel'))] = {'level': cast(int, d.get('level') or 0), 'worst': cw if cw is not None else sw,
                                       'basis': 'captured' if cw is not None else 'synthetic', 'captured_worst': cw,
                                       'captured_site': cs, 'synthetic_worst': sw, 'synthetic_site': ss,
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


def look(d: tuple[str, float] | None) -> str:
    if d is None:
        return ''
    kind, value = d
    if kind == 'ssim':
        return next(words for floor, words in LOOKS['ssim'] if value >= floor)
    return next(words for ceiling, words in LOOKS['worst_eighth'] if value <= ceiling)


def operands(corpus: Path, site: str) -> dict[str, object]:
    """A capture's facts (`exact`, the kept heads, the keys' crest per head) and each operand's worst head max/RMS, read
    from the corpus (BF16; numpy when present)."""
    path = corpus / site
    if not path.exists():
        return {}
    import struct
    with path.open('rb') as f:
        n = struct.unpack('<Q', f.read(8))[0]
        head = json.loads(f.read(n))
        data = f.read()
    facts = json.loads(head.get('__metadata__', {}).get('facts', '{}'))
    found: dict[str, object] = {'exact': facts.get('exact'), 'level': facts.get('level'), 'heads': facts.get('heads')}
    try:
        import numpy as np
    except ImportError:
        return found
    for name in ('query', 'key', 'value'):
        t = head[name]
        lo, hi = t['data_offsets']
        x = (np.frombuffer(data[lo:hi], dtype=np.uint16).astype(np.uint32) << 16).view(np.float32).reshape(t['shape'])
        per = [float(np.abs(x[:, :, h]).max() / max(float(np.sqrt(np.square(x[:, :, h]).mean())), 1e-12)) for h in range(x.shape[2])]
        found[f'{name}_crest'] = round(max(per), 1)
    return found


def main(argv: list[str]) -> int:
    corpus: Path | None = None
    if len(argv) > 3 and argv[2] == '--corpus':
        corpus = Path(argv[3])
        argv = argv[:2] + argv[4:]
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
                        'metrics': {'end_to_end': dict([d]) if d else None, 'micro': m}, 'looks': look(d), 'pass': e2e})
    kernels: dict[str, dict[str, dict[str, object]]] = {}
    for klass, found in micros.items():
        for kernel, m in found.items():
            bar, w = MICRO.get(m['level']), m['worst']
            micro_ok = None if bar is None or w is None else m['finite'] and w <= bar
            e2e = e2e_ok.get((klass, kernel))
            kernels.setdefault(klass, {})[kernel] = {'level': m['level'], 'micro': m, 'micro_bar': bar, 'micro_pass': micro_ok,
                                                     'end_to_end_pass': e2e,
                                                     'pass': micro_ok if e2e is None else bool(micro_ok) and e2e}
    manifest = {'bars': {'micro_worst_head_rel_l2': {str(k): v for k, v in MICRO.items()},
                         'S_8_min_layout_ssim': s8, 'E_8_max_worst_eighth': e8,
                         'how': "S_8/E_8: today's choice's worst distance from 16 bits, rounded outward to 0.005; micro: "
                                "measurement-design §3.2's starting bars on the captured corpus (each kernel's basis says "
                                "whether it had one), the synthetic sites' worst beside it"},
                'kernels': kernels, 'outputs': outputs}
    (out / 'manifest.json').write_text(json.dumps(manifest, indent=1, sort_keys=True) + '\n')
    (out / 'real-data.md').write_text(real_data(argv[2:], outputs, corpus))
    print(f"{len(outputs)} outputs, {sum(len(k) for k in kernels.values())} kernels in {len(kernels)} classes; S_8 {s8}, E_8 {e8}")
    return 0


def real_data(gates: list[str], outputs: list[dict[str, object]], corpus: Path | None) -> str:
    """Every captured site where an 8-bit kernel misses its bar, worst first, with its model's pictures."""
    lines = ['# Real-data errors over the 8-bit bar', '',
             f"Worst-head relative L2 against FP32 on the captured corpus; the 8-bit bar is {MICRO[8]}. `exact`: the Runtime "
             "holds the site to 16 bits (an exact path, or #2118's outlier-key rule). Crest: an operand's worst head max/RMS.", '']
    last: dict[str, Path] = {}  # a class's quality comes from its last gate directory given (a rerun's)
    for gate in map(Path, gates):
        if any(gate.glob('attention-quality-*.json')):
            last[str(detail(gate / 'attention-inventory.json').get('klass', ''))] = gate
    for klass, gate in sorted(last.items()):
        for path in sorted(gate.glob('attention-quality-*.json')):
            d = detail(path)
            if d.get('level') != 8:
                continue
            kernel = str(d.get('kernel'))
            errors = [e for e in cast(list[dict[str, object]], d.get('errors') or [])
                      if e.get('kind') == 'captured' and cast(float, e.get('worst') or 0.0) > MICRO[8]]
            by_site: dict[str, list[dict[str, object]]] = {}  # a site's captures: `<model>/<run>/<site>.<call>.safetensors`
            for e in errors:
                by_site.setdefault(str(e.get('site', '')).rsplit('.', 2)[0], []).append(e)
            for site, found in sorted(by_site.items(), key=lambda kv: -max(cast(float, e.get('worst') or 0.0) for e in kv[1])):
                found.sort(key=lambda e: -cast(float, e.get('worst') or 0.0))
                facts = operands(corpus, str(found[0].get('site', ''))) if corpus else {}
                crest = ', '.join(f"{n[0].upper()} {facts[f'{n}_crest']}" for n in ('query', 'key', 'value') if f'{n}_crest' in facts)
                calls = ', '.join(f"call {str(e.get('site', '')).rsplit('.', 2)[1].lstrip('0') or '0'}: {cast(float, e.get('worst')):.3f}" for e in found)
                lines.append(f"## {klass} · {kernel} · {site}")
                lines.append(f"- worst head: {calls}")
                lines.append(f"- level {found[0].get('level')}; exact: {facts.get('exact', 'unknown (no --corpus)')}; kept heads "
                             f"{facts.get('heads', '?')}; crest of the worst call's heads: {crest or 'n/a'}")
                package = PACKAGE.get(site.split('/')[0], '')
                for o in outputs:
                    if o['class'] == klass and o['model'] == package and o['kernel'] in ('16-bit reference', "today's choice (no pin)", kernel):
                        held = o['metrics']
                        metric = held.get('end_to_end') if isinstance(held, dict) else None
                        lines.append(f"- {o['cell']} {o['kernel']}: [{o['file']}]({o['file']}) {json.dumps(metric) if metric else ''} {o['looks']}")
                lines.append('')
    return '\n'.join(lines) + '\n'


if __name__ == '__main__':
    raise SystemExit(main(sys.argv))
