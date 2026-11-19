"""compare.py <lane> <digest A> <digest B> [max_buffer_bytes]: two arms' writes of one lane, tensor by tensor; one JSON line.

Every tensor of both commits by its parts' SHA-256, logical dtype, shape, scheme and params; each component's config and assets.
Beside it, where each quantized tensor's later parts (scales, codebooks) were staged at `max_buffer_bytes` (the Runtime passes
8 << 20, cozy-runtime derive/native_quantization.py), by TensorFS's `quantize` arithmetic (crates/tensorfs/src/session/quantize.rs:
`rows_per_chunk`, `later_in_memory`): in memory, or in scratch files. Exit 1 on any difference."""

from __future__ import annotations

import json
import sys


def staged(scheme: str, rows: int, cols: int, width: int, budget: int) -> tuple[str, int] | None:
    """(`memory` or `file`, rows per chunk) for one tensor, as quantize.rs plans it from a plain source of `width` bytes."""
    if scheme == "int8.convrot":
        row_output, scratch, later = cols + 4, cols * 8 + 4096, 4 * rows
    elif scheme == "nvfp4":
        row_output, scratch, later = cols // 2 + cols // 16, 4096, rows * (cols // 16) + 4
    elif scheme == "w4a8.convrot":
        row_output = cols // 2 + cols // 16 + 4
        scratch, later = cols * 8 + cols // 2 + 4096, rows * (cols // 16) + 4 * rows + 16 * 4
    else:
        return None
    row_memory = cols * (width + 4) + row_output
    per_chunk = min(max(budget - scratch, 0) // row_memory, rows)
    chunks = scratch + per_chunk * row_memory
    return ("memory" if chunks + later <= budget else "file"), per_chunk


def main() -> int:
    lane, a_path, b_path, *rest = sys.argv[1:]
    budget = int(rest[0]) if rest else 8 << 20
    a, b = (json.load(open(path)) for path in (a_path, b_path))
    ta, tb = a["tensors"], b["tensors"]
    same = lambda x, y: {k: v for k, v in x.items() if k != "source"} == {k: v for k, v in y.items() if k != "source"}  # noqa: E731
    differ = sorted(k for k in ta.keys() & tb.keys() if not same(ta[k], tb[k]))
    schemes: dict[str, int] = {}
    where: dict[str, dict[str, int]] = {}
    multi = unjudged = 0
    for key, row in tb.items():
        schemes[row["scheme"]] = schemes.get(row["scheme"], 0) + 1
        source = row.get("source") or {}
        if row["scheme"] == "plain" or len(row["shape"]) != 2 or source.get("encoded", True):
            unjudged += row["scheme"] != "plain"
            continue
        rows, cols = row["shape"]
        plan = staged(row["scheme"], rows, cols, 4 if source["dtype"] == "F32" else 2, budget)
        if plan is None:
            unjudged += 1
            continue
        kept, per_chunk = plan
        where.setdefault(row["scheme"], {"memory": 0, "file": 0})[kept] += 1
        multi += per_chunk < rows
    components = {n: a["components"].get(n) == b["components"].get(n) for n in a["components"].keys() | b["components"].keys()}
    meta = sorted(k for k in a["metadata"].keys() | b["metadata"].keys() if a["metadata"].get(k) != b["metadata"].get(k))
    total = {"memory": sum(s["memory"] for s in where.values()), "file": sum(s["file"] for s in where.values())}
    report = {
        "lane": lane, "a": a["ref"], "b": b["ref"], "same_header": a["id"] == b["id"], "tensors": len(ta),
        "identical": len(ta.keys() & tb.keys()) - len(differ), "differ": differ,
        "only_a": sorted(ta.keys() - tb.keys()), "only_b": sorted(tb.keys() - ta.keys()), "schemes": schemes,
        "max_buffer_bytes": budget, "staged": total, "staged_by_scheme": where, "multi_chunk": multi,
        "unjudged": unjudged, "components_equal": components, "metadata_differs": meta,
    }  # fmt: skip
    print(json.dumps(report))
    ok = not differ and not report["only_a"] and not report["only_b"] and all(components.values())
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
