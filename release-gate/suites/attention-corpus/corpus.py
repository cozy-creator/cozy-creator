#!/usr/bin/env python3
"""The attention corpus (xs-171 M-B): what a model's capture keeps, picked from its census, and what the captures hold.

    corpus.py pick DIR CENSUS        the `--attention-capture` spec: DIR, the chosen sites and calls, and heads
    corpus.py check CORPUS           every capture against its census row, and one short site's BF16 SDPA against FP32
    corpus.py summary CORPUS         per model: calls, sites, shapes, captures and bytes (markdown)

Sites: the first, middle and last of each block family (a site path with its block numbers taken out), and every site
whose calls differ from its family's usual site (other kernels, another number of calls, or other layout phases: H3's
exact paths, which also run dense through the sparse steps). Calls: the first and last of each layout phase (dense,
sparse) of those sites, or the first, middle and last when they have none. A pick over BUDGET keeps 2 heads, then only
each phase's first call, then 1 head. CORPUS holds one directory per model, the capture run's `<request id>/` beneath it
(cozy_runtime's attention_capture.py)."""

from __future__ import annotations

import json
import math
import struct
import sys
from collections import Counter
from collections.abc import Iterator
from pathlib import Path
from typing import TYPE_CHECKING, TypedDict, cast

if TYPE_CHECKING:  # pick and summary run on any python3; check under `uv run --with numpy --with torch`
    import numpy as np
    import numpy.typing as npt

BUDGET = 2 << 30  # bytes a model's capture may write (four models must fit a runner's disk)


class Layout(TypedDict, total=False):
    step: int
    sparse: bool


class Row(TypedDict, total=False):
    """One attention call (attention_capture.py's census row)."""
    site: str
    call: int
    backend: str
    band: str
    query: list[int]  # (batch, length, heads, head dim)
    key: list[int]
    dtype: str
    layout: Layout
    file: str


def rows(path: Path) -> list[Row]:
    return [cast(Row, json.loads(line)) for line in path.read_text().splitlines() if line.strip()]


def family(site: str) -> tuple[str, tuple[int, ...]]:
    """`transformer/transformer_blocks.12.attn` -> (`transformer/transformer_blocks.#.attn`, (12,))."""
    parts = site.split('.')
    return '.'.join('#' if p.isdigit() else p for p in parts), tuple(int(p) for p in parts if p.isdigit())


def phase(r: Row) -> bool | None:
    layout = r.get('layout')
    return layout.get('sparse') if layout else None


def size(r: Row, heads: int) -> int:
    """A capture's bytes: BF16 Q, K and V of HEADS heads (K/V with fewer heads than Q are kept whole)."""
    (b, lq, hq, d), (_, lk, hk, _) = r['query'], r['key']
    kept = min(heads, hq)
    return 2 * b * d * (lq * kept + 2 * lk * (kept if hk == hq else hk))


def pick(where: str, census: list[Row]) -> dict[str, object]:
    by_site: dict[str, list[Row]] = {}
    for r in census:
        if r.get('site'):
            by_site.setdefault(r['site'], []).append(r)
    families: dict[str, list[str]] = {}
    for site in by_site:
        families.setdefault(family(site)[0], []).append(site)
    def calls_of(site: str) -> tuple[tuple[str, ...], int, tuple[str, ...]]:
        """What a site's calls look like: its kernels, how many, and its layout phases."""
        rs = by_site[site]
        return tuple(sorted({r.get('backend', '') for r in rs})), len(rs), tuple(sorted({str(phase(r)) for r in rs}))

    chosen: set[str] = set()
    for sites in families.values():
        sites.sort(key=lambda s: family(s)[1])
        chosen |= {sites[0], sites[len(sites) // 2], sites[-1]}
        usual = Counter(calls_of(s) for s in sites).most_common(1)[0][0]
        chosen |= {s for s in sites if calls_of(s) != usual}
    phases: dict[bool | None, set[int]] = {}
    for site in chosen:
        for r in by_site[site]:
            phases.setdefault(phase(r), set()).add(r['call'])
    ends = {o for ordinals in phases.values() for o in (min(ordinals), max(ordinals))}
    calls = ends | ({sorted(phases[None])[len(phases[None]) // 2]} if list(phases) == [None] else set())
    firsts = {min(ordinals) for ordinals in phases.values()}
    heads, keep, total = 4, calls, 0
    for heads, keep in ((4, calls), (2, calls), (2, firsts), (1, firsts)):
        total = sum(size(r, heads) for s in chosen for r in by_site[s] if r['call'] in keep)
        if total <= BUDGET:
            break
    print(f'{len(chosen)} sites of {len(by_site)}, calls {sorted(keep)}, {heads} heads: {total / 2**30:.2f} GiB', file=sys.stderr)
    return {'dir': where, 'sites': sorted(chosen), 'calls': sorted(keep), 'heads': heads, 'seed': 171}


def tensors(path: Path) -> tuple[dict[str, str], dict[str, npt.NDArray[np.float32]]]:
    """A capture's facts and its BF16 tensors as float32 (safetensors' layout, read here: no safetensors needed)."""
    import numpy as np

    with path.open('rb') as f:
        n = struct.unpack('<Q', f.read(8))[0]
        head = json.loads(f.read(n))
        data = f.read()
    found: dict[str, npt.NDArray[np.float32]] = {}
    for name, t in head.items():
        if name == '__metadata__':
            continue
        if t['dtype'] != 'BF16':
            raise ValueError(f'{path.name}: {name} is {t["dtype"]}, not BF16')
        lo, hi = t['data_offsets']
        bits = np.frombuffer(data[lo:hi], dtype=np.uint16).astype(np.uint32) << 16
        found[name] = bits.view(np.float32).reshape(t['shape'])
    return cast(dict[str, str], head['__metadata__']), found


def captures(corpus: Path) -> Iterator[tuple[str, Path, list[Row]]]:
    """(model, request dir, its census) for every capture run under CORPUS."""
    for census in sorted(corpus.glob('*/*/census.jsonl')):
        yield census.parent.parent.name, census.parent, rows(census)


def check(corpus: Path) -> int:
    import numpy as np
    import torch

    fails: list[str] = []
    if not any(True for _ in captures(corpus)):
        fails.append(f'no capture run under {corpus}')
    for model, run, census in captures(corpus):
        kept = [r for r in census if r.get('file')]
        if not kept:
            print(f'{model}: no capture in {run.name} ({len(census)} calls in its census)')
            continue
        for r in kept:
            meta, t = tensors(run / r['file'])
            heads = len(json.loads(meta['facts'])['heads'])
            hq = r['query'][2]
            for name, call in (('query', r['query']), ('key', r['key']), ('value', r['key'])):
                want = [*call[:2], heads if name == 'query' or call[2] == hq else call[2], call[3]]
                if list(t[name].shape) != want:
                    fails.append(f"{model} {r['file']}: {name} is {list(t[name].shape)}, its call {want}")
                if not np.isfinite(t[name]).all():
                    fails.append(f"{model} {r['file']}: {name} holds non-finite values")
        # One short site's attention: BF16 SDPA against FP32, per head (the suite's quality cells' metric).
        r = min(kept, key=lambda r: r['query'][1] * r['key'][1])
        _, t = tensors(run / r['file'])
        q, k, v = (torch.from_numpy(t[n]).transpose(1, 2) for n in ('query', 'key', 'value'))  # (B, H, L, D)
        if k.shape[1] != q.shape[1]:
            k, v = (x.repeat_interleave(q.shape[1] // x.shape[1], dim=1) for x in (k, v))
        sdpa = torch.nn.functional.scaled_dot_product_attention
        exact = sdpa(q, k, v)
        half = sdpa(q.bfloat16(), k.bfloat16(), v.bfloat16()).float()
        err = ((half - exact).norm(dim=(0, 2, 3)) / exact.norm(dim=(0, 2, 3))).tolist()
        flat = ((v.mean(dim=2, keepdim=True).expand_as(exact) - exact).norm() / exact.norm()).item()
        print(f"{model}: {len(kept)} captures; {r['site']} call {r['call']} {r['query']}x{r['key'][1]}: BF16 SDPA vs FP32 "
              f"worst head {max(err):.4f}; the mean of V is {flat:.3f} from FP32")
        if not all(math.isfinite(e) and e <= 0.01 for e in err):
            fails.append(f"{model} {r['file']}: BF16 SDPA vs FP32 {max(err):.4f}, over BF16 rounding (0.01)")
    for f in fails:
        print(f'FAIL {f}')
    return 1 if fails else 0


def summary(corpus: Path) -> None:
    print('| model | calls | sites | Lq x Lk, heads x D (calls) | captures | MiB |\n|---|---|---|---|---|---|')
    for model, run, census in captures(corpus):
        shapes = Counter(f"{r['query'][1]}x{r['key'][1]}, {r['query'][2]}x{r['query'][3]}" for r in census if r.get('query'))
        files = [run / r['file'] for r in census if r.get('file')]
        print(f"| {model} | {len(census)} | {len({r.get('site') for r in census})} | "
              f"{'; '.join(f'{s} ({n})' for s, n in shapes.most_common(4))} | {len(files)} | "
              f"{sum(f.stat().st_size for f in files) / 2**20:.0f} |")


def main(argv: list[str]) -> int:
    if len(argv) == 4 and argv[1] == 'pick':
        print(json.dumps(pick(argv[2], rows(Path(argv[3]))), separators=(',', ':')))
        return 0
    if len(argv) == 3 and argv[1] == 'check':
        return check(Path(argv[2]))
    if len(argv) == 3 and argv[1] == 'summary':
        summary(Path(argv[2]))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == '__main__':
    raise SystemExit(main(sys.argv))
