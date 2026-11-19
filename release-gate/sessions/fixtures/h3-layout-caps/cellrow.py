"""cellrow.py LEDGER_PY RECORD_JSON: one run record's numbers as a JSON line, read as the benchmark ledger reads them
(ledger.py `phases`, `measurements`): status, memoized, execution, refine / upscale_latent / decode_video seconds, the refine's
step 1 and steady step, peak driver VRAM (GiB), SM clock, and the Runtime and TensorFS each call ran on."""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(sys.argv[1]))
from ledger import arr, load, measurements, num, obj, phases, text, throttled  # noqa: E402

rec = obj(load(sys.argv[2]))
ph = phases(rec)
ms = measurements(rec)
stages = obj(ph.get("stages"))
drivers = [obj(d) for m in ms for d in arr(obj(m.get("memory")).get("driver"))]
soft: dict[str | None, str | None] = {}
for m in ms:
    s = obj(m.get("software"))
    if s:
        soft[text(s.get("runtime"))] = text(s.get("tensorfs"))
memo = obj(rec.get("memo")).get("memoized") is True or any(obj(c).get("memoized") is True for c in arr(rec.get("calls")))
print(json.dumps({
    "status": text(rec.get("status")), "memoized": memo, "execution_s": round((num(rec.get("execution_ms")) or 0) / 1000, 1),
    "refine_s": num(stages.get("refine")), "upscale_latent_s": num(stages.get("upscale_latent")),
    "decode_s": num(stages.get("decode_video")), "encode_s": num(stages.get("encode_video")), "step1_s": num(ph.get("step1_s")),
    "steady_s": num(ph.get("steady_s")), "steps": num(ph.get("steps")),
    "peak_vram_gib": round(max([num(d.get("peak_bytes")) or 0 for d in drivers] or [0]) / 2**30, 2),
    "sm_mhz_mean": max([num(d.get("sm_mhz_mean")) or 0 for d in drivers] or [0]), "throttled": throttled(rec),
    "software": [[r, t] for r, t in soft.items()],
}))
