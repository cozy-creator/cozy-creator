"""Suite `kernels` (./cells.sh): every entry and jit function the inventory declared for this card was launched by a cell. A gate run with
--only judges only the kernels it chose: each chosen cell ran, and the rest are noted as not chosen, not failed.
    check(gate) -> (fails, notes)"""
import json
import os


def _detail(run):
    try:
        return json.loads((run.result or {}).get('detail') or '{}')
    except (TypeError, ValueError, AttributeError):
        return {}


def check(gate):
    fails, notes, runs = [], [], gate['runs']
    if 'kernels-inventory' not in runs:
        return fails, notes
    found = _detail(runs['kernels-inventory'])
    if not found.get('names'):
        fails.append('kernels-inventory: no inventory')
        return fails, notes
    cells = {name: _detail(run) for name, run in runs.items() if name.startswith(('kernel-', 'kernel1-'))}
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
