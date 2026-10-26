# /// script
# requires-python = ">=3.12"
# [tool.cozy]
# accelerator = false
# ///
"""On the rental, holding no GPU itself: what the card holds now (nvidia-smi totals and compute apps, with each
app's command line) and the host's memory. The GPU-free check before an engine's arm, and what an idle TensorD
holds. Returns its record as the value."""
import json
import subprocess
import time
from pathlib import Path
from cozy_runtime.author import Context, Telemetry


def smi(query: str) -> list[list[str]]:
    out = subprocess.run(["nvidia-smi", f"--query-{query}", "--format=csv,noheader,nounits"],
                         capture_output=True, text=True).stdout
    return [[x.strip() for x in line.split(",")] for line in out.splitlines() if line.strip()]


def cmdline(pid: str) -> str:
    try:
        return open(f"/proc/{pid}/cmdline", "rb").read().replace(b"\0", b" ").decode(errors="replace")[:300]
    except OSError:
        return "?"


def main(ctx: Context, tel: Telemetry) -> str:
    gpu = smi("gpu=name,memory.used,memory.total,utilization.gpu,temperature.gpu,clocks.sm,power.draw")
    apps = [{"pid": p, "name": n, "used_mib": u, "cmdline": cmdline(p)}
            for p, n, u in smi("compute-apps=pid,process_name,used_memory")]
    meminfo = {line.split(":")[0]: int(line.split()[1]) for line in open("/proc/meminfo")
               if line.split(":")[0] in ("MemTotal", "MemAvailable", "Cached")}
    used = int(gpu[0][1]) if gpu else -1
    record = {"uuid": smi("gpu=uuid,pci.bus_id,name,driver_version"), "unix_s": time.time(),
              "gpu": gpu, "used_mib": used, "apps": apps, "meminfo_kib": meminfo,
              "free": 0 <= used < 1024}
    tel.log(f"GPU used {used} MiB, {len(apps)} compute apps: {[(a['pid'], a['used_mib'], a['cmdline'][:80]) for a in apps]}")
    return json.dumps(record)
