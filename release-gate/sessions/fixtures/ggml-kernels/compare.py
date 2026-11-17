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
    for path in benches:
        speed = json.loads(json.load(open(path))["detail"])
        for shape in speed["shapes"]:
            rules = shape["rules"].get("ggml", {})
            for name, timed in shape["times"].items():
                head, _, kind = name.partition(":")
                if not head.startswith("ggml-matmul"):
                    continue
                per = float(timed["ms"][0]) * 1000 / int(head.removeprefix("ggml-matmul"))
                linear = shape["times"].get(name.replace("matmul", "linear"), {"ms": [0]})["ms"][0]
                ours[f"{shape['model']}-{shape['site']}-{kind}"] = (per, float(linear) * 1000 / int(head.removeprefix("ggml-matmul")), rules.get(f"kernel:{kind}", ""))
    rows, misses = [], []
    print(f"card copy {gbps:.0f} GB/s, L2 {l2 >> 20} MiB\n")
    print("| case | kernel | llama.cpp us | ours us | ours/llama | leaf us | weight GB/s | % copy |")
    print("|---|---|---|---|---|---|---|---|")
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
        print(f"| {case} | {kernel} | {theirs[case]:.1f} | {us:.1f} | {ratio:.3f} | {leaf:.1f} | {rate:.0f}{' (L2)' if resident else ''} | {100 * rate / gbps:.0f}% |")
        if ratio > 1 / 0.95:
            misses.append(f"{case}: ours {us:.1f} us, llama.cpp {theirs[case]:.1f} us ({ratio:.3f}x)")
        if bandwidth_bar and rate < 0.75 * gbps:
            misses.append(f"{case}: {rate:.0f} GB/s, under 75% of copy {gbps:.0f}")
        rows.append({"case": case, "kernel": kernel, "llama_us": theirs[case], "ours_us": round(us, 2), "ratio": round(ratio, 4),
                     "leaf_us": round(leaf, 2), "gbps": round(rate, 1)})
    print()
    print(json.dumps({"copy_gbps": gbps, "rows": len(rows), "missed": misses}))
    return 1 if misses else 0


if __name__ == "__main__":
    sys.exit(main())
