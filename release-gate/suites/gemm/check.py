"""Suite `gemm` (./cells.sh, xs-177): each lane's GEMM rule against every candidate the Runtime could serve, at our
packages' shapes. The rules are fitted once and drift when kernels, torch or cuBLAS move (torch 2.15's cuBLASLt overtook
Hopper's fp8 tiles), so a rule whose pick is more than DRIFT slower than the best candidate, time-weighted over one
model's shapes and costing at least COST a request, fails; so does a tile whose output bits differ from the rule's. A
single shape over DRIFT is a note.
    check(gate) -> (fails, notes)"""
import json
from collections import defaultdict
from typing import TypedDict, cast

from runs import GateRuns, Run

#: Twice and more the timing's noise floor (config -1 against its own explicit tile: under 4% on a healthy pod).
DRIFT = 0.10
#: Seconds a request a drift must cost to fail: an LM layer's 64 calls drift past DRIFT at a millisecond (gate 43104499783).
COST = 0.1


class Timed(TypedDict):
    ms: list[float]


class Shape(TypedDict, total=False):
    id: str
    model: str
    calls: int
    rules: dict[str, dict[str, str]]
    times: dict[str, Timed]
    same_bits: dict[str, bool]


def _detail(run: Run) -> dict[str, object]:
    try:
        # A result that is not an object, or a detail that is not text, raises: caught below.
        return json.loads((run.result or {}).get('detail') or '{}')  # type: ignore[union-attr, arg-type, no-any-return]
    except (TypeError, ValueError, AttributeError):
        return {}


def check(gate: GateRuns) -> tuple[list[str], list[str]]:
    fails: list[str] = []
    notes: list[str] = []
    weighted: dict[tuple[str, str, str], list[float]] = defaultdict(lambda: [0.0, 0.0])
    card = ''
    for name, run in sorted(gate['runs'].items()):
        if not name.startswith('gemm-speed-'):
            continue
        found = _detail(run)
        card = card or str(cast(dict[str, object], found.get('inventory') or {}).get('card', ''))
        for shape in cast(list[Shape], found.get('shapes') or []):
            times = shape.get('times') or {}
            bad = sorted(k for k, same in (shape.get('same_bits') or {}).items() if not same)
            if bad:
                fails.append(f"{shape.get('id')}: tiles whose output bits differ from the rule's: {', '.join(bad)}")
            for lane, rule in (shape.get('rules') or {}).items():
                for level in ('gemm', 'linear'):
                    pick = rule.get(level, '')
                    prefix = f'{lane}-{level}:'
                    if not pick.startswith(prefix) or pick not in times:
                        continue
                    others = {k: v['ms'][0] for k, v in times.items() if k.startswith(prefix) and k != pick}
                    if not others:
                        continue
                    best = min(others, key=others.__getitem__)
                    mine = times[pick]['ms'][0]
                    calls = shape.get('calls', 1)
                    total = weighted[(shape.get('model', ''), lane, level)]
                    total[0] += calls * mine
                    total[1] += calls * min(mine, others[best])
                    if mine > (1 + DRIFT) * others[best]:
                        notes.append(f"{shape.get('id')} {lane} {level}: the rule's {pick.split(':')[1]} {mine:.3f} ms, "
                                     f"{best.split(':')[1]} {others[best]:.3f} ms (+{mine / others[best] - 1:.0%})")
    for (model, lane, level), (mine, best_ms) in sorted(weighted.items()):
        if best_ms > 0 and mine > (1 + DRIFT) * best_ms and (mine - best_ms) / 1000 >= COST:
            fails.append(f'{card} {model} {lane} {level}: the rule is {mine / best_ms - 1:.0%} slower than the best candidate '
                         f'over its shapes ({(mine - best_ms) / 1000:.2f} s a request): refit it (xs-177)')
    return fails, notes
