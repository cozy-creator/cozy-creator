"""ggml-kernels (xs-213): our engine's llama.cpp matmuls against stock llama.cpp's on the same card and cases.

    compare.py <llama-perf.txt> <calibrate.json> <bench.json>... > summary

Ours: the GEMM bench's `ggml-matmul<G>:<type>` (G back-to-back matmuls from f32 rows, the q8_1 quantize included, graphed
as test-backend-ops runs one op), per matmul; `ggml-linear` the leaf's whole path from bf16 rows. Theirs: test-backend-ops
perf's us/run. Each row: the kernel ggml's rule took, both times, ours over theirs (the bar: ours at most 1/0.95 of
theirs), and the weight's GB/s against the pod's copy GB/s (the bar for one row: 75%, on weights larger than L2).
Prints a markdown table, then one JSON line of the verdicts; exits 1 on a bar missed.
"""

import json
import re
import sys

from shapes import TYPES, cases

PERF = re.compile(r"MUL_MAT\(name=([^,]+),.*?\):\s+(\d+) runs -\s+([\d.]+) us/run")


def main() -> int:
    perf, calibrate, *benches = sys.argv[1:]
    theirs = {m.group(1): float(m.group(3)) for m in PERF.finditer(open(perf).read())}
    copy = json.loads(json.load(open(calibrate))["detail"])
    gbps, l2 = float(copy["gbps"]), int(copy["inventory"]["l2"])
    ours: dict[str, tuple[float, float, str]] = {}
    int8: dict[str, float] = {}
    # The bench's `ordered:<type>` (f32 rows) and `ordered-linear:<type>` (bf16 rows) rows: a read on torch's default
    # stream queued right after a call, unsynced, sees its output (cozy-runtime#2309). Any other value misses.
    ordered: dict[str, str] = {}
    for path in benches:
        speed = json.loads(json.load(open(path))["detail"])
        for shape in speed["shapes"]:
            rules = shape["rules"].get("ggml", {})
            for rule, said in rules.items():
                if rule.startswith("ordered"):
                    ordered[f"{shape['model']}-{shape['site']} {rule}"] = said
            for name, timed in shape["times"].items():
                head, _, kind = name.partition(":")
                if head.startswith("ggml-int8"):
                    int8[f"{shape['model']}-{shape['site']}"] = float(timed["ms"][0]) * 1000 / int(head.removeprefix("ggml-int8"))
                if not head.startswith("ggml-matmul"):
                    continue
                per = float(timed["ms"][0]) * 1000 / int(head.removeprefix("ggml-matmul"))
                linear = shape["times"].get(name.replace("matmul", "linear"), {"ms": [0]})["ms"][0]
                ours[f"{shape['model']}-{shape['site']}-{kind}"] = (per, float(linear) * 1000 / int(head.removeprefix("ggml-matmul")), rules.get(f"kernel:{kind}", ""))
    rows, misses = [], []
    print(f"card copy {gbps:.0f} GB/s, L2 {l2 >> 20} MiB\n")
    print("| case | kernel | llama.cpp us | ours us | ours/llama | leaf us | weight GB/s | % copy | int8.convrot us (GB/s) |")
    print("|---|---|---|---|---|---|---|---|---|")
    for case, model, site, m, n, k, name, _, block, size in cases():
        key = f"{model}-{site}-{name}"
        if case not in theirs or key not in ours:
            misses.append(f"{case}: unmeasured ({'llama' if case not in theirs else 'ours'})")
            continue
        us, leaf, kernel = ours[key]
        weight = n * (k // block) * size
        rate = weight / (us * 1e-6) / 1e9
        ratio = us / theirs[case]
        resident = weight <= l2
        bandwidth_bar = kernel == "mmvq" and m == 1 and not resident
        mine = int8.get(f"{model}-{site}")
        eight = f"{mine:.1f} ({n * k / (mine * 1e-6) / 1e9:.0f})" if mine else ""
        print(f"| {case} | {kernel} | {theirs[case]:.1f} | {us:.1f} | {ratio:.3f} | {leaf:.1f} | {rate:.0f}{' (L2)' if resident else ''} | {100 * rate / gbps:.0f}% | {eight} |")
        if ratio > 1 / 0.95:
            misses.append(f"{case}: ours {us:.1f} us, llama.cpp {theirs[case]:.1f} us ({ratio:.3f}x)")
        if bandwidth_bar and rate < 0.75 * gbps:
            misses.append(f"{case}: {rate:.0f} GB/s, under 75% of copy {gbps:.0f}")
        rows.append({"case": case, "kernel": kernel, "llama_us": theirs[case], "ours_us": round(us, 2), "ratio": round(ratio, 4),
                     "leaf_us": round(leaf, 2), "gbps": round(rate, 1), "int8_us": round(mine, 2) if mine else None})
    late = sorted(k for k, said in ordered.items() if said != "true")
    misses += [f"{k}: a read on the default stream right after the call missed its output ({ordered[k]})" for k in late]
    print(f"\nordered: {len(ordered) - len(late)} of {len(ordered)} rows true, over "
          f"{len({k.split(' ')[0] for k in ordered})} shapes")
    print()
    print(json.dumps({"copy_gbps": gbps, "rows": len(rows), "ordered": len(ordered), "ordered_late": len(late), "missed": misses}))
    return 1 if misses else 0


if __name__ == "__main__":
    sys.exit(main())
