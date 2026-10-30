"""triton-prebuilt's bars (proof-only):
1. arm P's kernel boot, with no C compiler on the pod, readies the Runtime's Triton sets (copied from the wheel);
2. arm P's LoRA request completes and matches arm C's (with a compiler): the same bytes, else the same decoded image;
3. the package environment's Runtime carries the prebuilt sets and this Python's launcher.
Reported: each kernel boot's Triton sets and their times (arm C compiles them; arm P copies them).
Prints the verdict as JSON; exits 1 when a bar fails."""

import json
import sys
import zipfile
from pathlib import Path

out = Path(sys.argv[1])
with zipfile.ZipFile(next((out / "wheel-p").glob("cozy_runtime-*manylinux*x86_64.whl"))) as wheel:
    SETS = {k["set"] for k in json.loads(wheel.read("cozy_runtime/_triton/manifest.json"))["kernels"].values()}


def boots(name, after=0):
    rows = []
    path = out / name
    for line in (path.read_text().splitlines() if path.exists() else [])[after:]:
        try:
            b = json.loads(line)
        except ValueError:
            continue
        rows.append({"generation": b.get("generation"), "took_s": round((b.get("took_ms") or 0) / 1000, 1),
                     "sets": {k["kernel"]: {"state": k.get("state"), "s": round(float(k.get("compile_ms") or 0) / 1000, 2),
                                            "line": str(k.get("line", ""))[:80]}
                              for k in b.get("kernels") or [] if isinstance(k, dict) and k.get("kernel") in SETS}})
    return rows


def run(label):
    path = out / f"{label}.json"
    if not path.exists():
        return {"status": "no run"}
    r = json.loads(path.read_text())
    ready = sorted({str(o.get("value")) for c in r.get("calls") or [] for o in (c.get("measurements") or {}).get("observations") or []
                    if o.get("name") == "kernel.ready"})
    return {"status": r.get("status"), "exec_s": round((r.get("execution_ms") or 0) / 1000, 1), "kernel_ready": ready}


c_lines = len((out / "kernel-boot-c.jsonl").read_text().splitlines()) if (out / "kernel-boot-c.jsonl").exists() else 0
verdict = {
    "boots": {"c": boots("kernel-boot-c.jsonl"), "p": boots("kernel-boot-p.jsonl", c_lines)},
    "requests": {r: run(r) for r in ("c1", "p1")},
    "ab": (out / "ab.txt").read_text().split("\n") if (out / "ab.txt").exists() else [],
    "package_env": json.loads((out / "package-env.json").read_text()) if (out / "package-env.json").exists() else None,
    "cc_off": json.loads((out / "cc-off.json").read_text()) if (out / "cc-off.json").exists() else None,
    "failed": [],
}
failed = verdict["failed"]
p_sets = {k: v for b in verdict["boots"]["p"] for k, v in b["sets"].items()}
if "cozy-lora-bake" not in p_sets or any(v["state"] != "ready" for v in p_sets.values()):
    failed.append(f"arm P's boot without a compiler: {p_sets or 'no Runtime set'}")
if verdict["requests"]["p1"].get("status") != "completed":
    failed.append(f"p1: {verdict['requests']['p1']}")
if not {"p1 identical bytes", "p1 identical decoded image"} & set(verdict["ab"]):
    failed.append(f"p1 against c1: {verdict['ab']}")
env = verdict["package_env"] or {}
if not env.get("shipped") or not env.get("kernels"):
    failed.append(f"package environment: {env}")
if (verdict["cc_off"] or {}).get("which_gcc") or (verdict["cc_off"] or {}).get("which_cc"):
    failed.append(f"a compiler stayed on PATH: {verdict['cc_off']}")
print(json.dumps(verdict, indent=1))
sys.exit(1 if failed else 0)
