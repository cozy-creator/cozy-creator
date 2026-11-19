"""layout.py SPLIT_PY REGENERATE_PY T H W: the refine's windows over (1, 24, T, H, W) latents, as the tree's `plan_windows`
lays them out with a row bound (`WINDOW_ROWS`), from the Runtime's own `author/_split.py`, as one JSON line:
{"most", "rows_per_frame", "spans": [[start, stop], ...], "overlaps": [[start, stop], ...]} in latent frames.
Pure Python: `_split.py` imports torch only for type checking (cr-343). A tree whose `WINDOW_ROWS` is None leaves the layout
to the card's memory: no spans ("most": null)."""

import ast
import importlib.util
import json
import sys

split_py, regenerate_py, t, h, w = sys.argv[1], sys.argv[2], *map(int, sys.argv[3:6])
spec = importlib.util.spec_from_file_location("split_rc", split_py)
assert spec and spec.loader
split = importlib.util.module_from_spec(spec)
sys.modules["split_rc"] = split
spec.loader.exec_module(split)

# The module's own constants, in order: names bound to int expressions of earlier ones.
known: dict[str, object] = {"None": None}
for node in ast.parse(open(regenerate_py).read()).body:
    target, value = None, None
    if isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
        target, value = node.targets[0].id, node.value
    elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name) and node.value is not None:
        target, value = node.target.id, node.value
    if target is None or value is None:
        continue
    try:
        known[target] = eval(compile(ast.Expression(value), regenerate_py, "eval"), {"__builtins__": {}}, dict(known))
    except Exception:
        pass
most, overlap, align = known.get("WINDOW_ROWS"), known["OVERLAP"], 5
rows = (h // 2) * (w // 2)
out: dict[str, object] = {"most": most, "rows_per_frame": rows}
if most is not None:
    assert isinstance(most, int) and isinstance(overlap, int)
    spans = split._fewest(t, split.Cut(0, overlap=overlap, align=align), True, most // rows)
    out["spans"] = [list(s) for s in spans]
    out["overlaps"] = [[b[0], a[1]] for a, b in zip(spans, spans[1:])]
    out["distinct"] = len(set(spans)) == len(spans)
print(json.dumps(out))
