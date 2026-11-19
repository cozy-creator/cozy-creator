"""cellrow.py LEDGER_PY RECORD_JSON: one run record's numbers as a JSON line, read as the benchmark ledger reads them
(ledger.py `phases`, `measurements`): status, memoized, execution, refine / upscale_latent / decode_video seconds, the refine's
step 1 and steady step, peak driver VRAM (GiB), SM clock, and the Runtime and TensorFS each call ran on."""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(sys.argv[1]))
import ledger  # noqa: E402

rec = json.load(open(sys.argv[2]))
ph = ledger.phases(rec)
ms = ledger.measurements(rec)
stages = ph.get("stages") or {}
drivers = [d for m in ms for d in (m.get("memory") or {}).get("driver") or []]
soft = {(m.get("software") or {}).get("runtime"): (m.get("software") or {}).get("tensorfs") for m in ms if m.get("software")}
memo = bool((rec.get("memo") or {}).get("memoized")) or any(c.get("memoized") for c in rec.get("calls") or [])
print(json.dumps({
    "status": rec.get("status"), "memoized": memo, "execution_s": round((rec.get("execution_ms") or 0) / 1000, 1),
    "refine_s": stages.get("refine"), "upscale_latent_s": stages.get("upscale_latent"), "decode_s": stages.get("decode_video"),
    "encode_s": stages.get("encode_video"), "step1_s": ph.get("step1_s"), "steady_s": ph.get("steady_s"), "steps": ph.get("steps"),
    "peak_vram_gib": round(max([d.get("peak_bytes") or 0 for d in drivers] or [0]) / 2**30, 2),
    "sm_mhz_mean": max([d.get("sm_mhz_mean") or 0 for d in drivers] or [0]), "throttled": ledger.throttled(rec),
    "software": [[r, t] for r, t in soft.items()],
}))
