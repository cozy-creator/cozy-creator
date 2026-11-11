"""Suite `kernels` (./cells.sh): every entry and jit function the inventory declared for this card was launched by a cell. A gate run with
--only judges only the kernels it chose: each chosen cell ran, and the rest are noted as not chosen, not failed.
    check(gate) -> (fails, notes)"""
import json
import os
from typing import TypedDict, cast

from runs import GateRuns, Run


class Inventory(TypedDict, total=False):
    """`kernels-inventory`'s `result.detail`: what this card declares (the Runtime's `bench.kernels_inventory`)."""
    names: list[str]
    row: str
    entries: dict[str, list[str]]  # per kernels set
    jits: dict[str, list[str]]  # per module


class Launched(TypedDict, total=False):
    """A kernel cell's `result.detail`: its verdict and what it launched (the Runtime's `bench.kernel_check`)."""
    passed: bool
    detail: str
    entries: list[str]
    jits: list[str]


def _detail(run: Run) -> Inventory | Launched:
    try:
        # A result that is not an object, or a detail that is not text, raises: caught below. The detail is cells.sh's document.
        return json.loads((run.result or {}).get('detail') or '{}')  # type: ignore[union-attr, arg-type, no-any-return]
    except (TypeError, ValueError, AttributeError):
        return {}


def check(gate: GateRuns) -> tuple[list[str], list[str]]:
    fails: list[str]
    notes: list[str]
    fails, notes, runs = [], [], gate['runs']
    if 'kernels-inventory' not in runs:
        return fails, notes
    found = cast(Inventory, _detail(runs['kernels-inventory']))  # the inventory cell's document
    if not found.get('names'):
        fails.append('kernels-inventory: no inventory')
        return fails, notes
    cells = {name: cast(Launched, _detail(run)) for name, run in runs.items() if name.startswith(('kernel-', 'kernel1-'))}
    try:
        chosen = {line.strip() for line in open(os.path.join(gate['out'], 'only')) if line.strip()}
    except OSError:
        chosen = set()
    if chosen:
        names = [name for name in found['names'] if 'kernel-' + name.replace(':', '-') in chosen]
        for name in names:
            if 'kernel-' + name.replace(':', '-') not in runs:
                fails.append(f'{name}: chosen, and no cell passed it on {found.get("row")}')
        notes.append(f'kernels: --only chose {len(names)} of {len(found["names"])}; the rest were not judged')
        for name, detail in cells.items():
            if detail.get('passed'):
                notes.append(f"{name}: {detail.get('detail', '')[:160]}")
        return fails, notes
    entries, jits = set(), set()
    for name, detail in cells.items():
        entries |= set(detail.get('entries', []))
        jits |= set(detail.get('jits', []))
        if detail.get('passed'):
            notes.append(f"{name}: {detail.get('detail', '')[:160]}")
    for name in found['names']:
        cell = 'kernel-' + name.replace(':', '-')
        if cell not in runs:
            fails.append(f'{name}: no cell passed it on {found.get("row")}')
    for kernels, keys in found.get('entries', {}).items():
        missing = sorted(set(keys) - entries)
        if missing:
            fails.append(f'{kernels}: {len(missing)} of {len(keys)} entries no cell launched: {missing[:6]}')
    for module, names in found.get('jits', {}).items():
        missing = sorted(set(names) - jits)
        if missing:
            fails.append(f'{module}: jit functions no cell launched on {found.get("row")}: {missing}')
    return fails, notes
