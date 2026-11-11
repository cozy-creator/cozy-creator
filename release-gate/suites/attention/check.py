"""Suite `attention` (./cells.sh): every kernel the inventory names was timed or said why not, and was judged. The bars are
the Runtime's (`measurements/rules.toml`, applied by `scripts/attention_table.py`): this lists what the judging measured.
A gate run with --only judges only the cells it chose.
    check(gate) -> (fails, notes)"""
import json
import os
from typing import TypedDict, cast

from runs import GateRuns, Run


class Inventory(TypedDict, total=False):
    klass: str
    card: str
    kernels: list[str]


class Point(TypedDict, total=False):
    times: dict[str, object]
    refused: dict[str, str]
    failed: dict[str, str]


class Speed(TypedDict, total=False):
    absent: dict[str, str]
    points: list[Point]


class Error(TypedDict, total=False):
    site: str
    worst: float
    finite: bool


class Quality(TypedDict, total=False):
    kernel: str
    errors: list[Error]


def _detail(run: Run) -> dict[str, object]:
    try:
        # A result that is not an object, or a detail that is not text, raises: caught below.
        return json.loads((run.result or {}).get('detail') or '{}')  # type: ignore[union-attr, arg-type, no-any-return]
    except (TypeError, ValueError, AttributeError):
        return {}


def check(gate: GateRuns) -> tuple[list[str], list[str]]:
    fails: list[str] = []
    notes: list[str] = []
    runs = gate['runs']
    if 'attention-inventory' not in runs:
        return fails, notes
    inventory = cast(Inventory, _detail(runs['attention-inventory']))
    kernels = inventory.get('kernels') or []
    if not kernels:
        return ['attention-inventory: no inventory'], notes
    try:
        chosen = {line.strip() for line in open(os.path.join(gate['out'], 'only')) if line.strip()}
    except OSError:
        chosen = set()
    speeds = [cast(Speed, _detail(run)) for name, run in sorted(runs.items()) if name.startswith('attention-speed-')]
    said: dict[str, str] = {}
    for speed in speeds:
        said.update(speed.get('absent') or {})
        for point in speed.get('points') or []:
            for k in point.get('times') or {}:
                said.setdefault(k, '')
            for key in ('failed', 'refused'):
                for k, why in (point.get(key) or {}).items():  # type: ignore[attr-defined]
                    said.setdefault(k, f'{key}: {why}')
    timed = {k for speed in speeds for point in speed.get('points') or [] for k in point.get('times') or {}}
    if not chosen or any(c.startswith('attention-speed') for c in chosen):
        for k in kernels:
            if k not in said:
                fails.append(f'{k}: on {inventory.get("card")} and no speed cell timed it or said why not')
            elif k not in timed:
                notes.append(f'{k}: not timed on {inventory.get("card")}: {said[k][:160]}')
    for k in kernels:
        cell = f'attention-quality-{k}'
        if chosen and cell not in chosen:
            continue
        if cell not in runs:
            fails.append(f'{k}: no quality cell completed')
            continue
        errors = cast(Quality, _detail(runs[cell])).get('errors') or []
        if errors:
            worst = max(errors, key=lambda e: e.get('worst', 0.0))
            finite = all(e.get('finite') for e in errors)
            notes.append(f'{k}: worst head {worst.get("worst", 0.0):.4f} at {worst.get("site")}{"" if finite else ", NON-FINITE output"}')
    notes.append(f'attention: class {inventory.get("klass")} on {inventory.get("card")}, {len(kernels)} kernels')
    return fails, notes
