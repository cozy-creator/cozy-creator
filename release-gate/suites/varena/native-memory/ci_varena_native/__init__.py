"""The varena suite's native-memory cell (xs-090 D1, D2, D4), run on the rented GPU through `cozy run`.

A ballast process (plain torch, no varena) holds all but `free_gib` of the card: another app's memory. Each stage
then runs in a fresh process of its own (child.py) whose standalone varena Memory gets what is left, so the cell needs
no cap flag and never touches this executor's own leased Memory. The fault stage poisons only its own process.
"""

import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import msgspec

from cozy_runtime.author import App, Telemetry

app = App()
CHILD = Path(__file__).with_name("child.py")
BALLAST = r"""
import sys, torch
free, total = torch.cuda.mem_get_info()
n = max(free - int(sys.argv[1]), 0)
x = torch.empty(n, dtype=torch.uint8, device="cuda") if n else None
print(n, total, flush=True)
sys.stdin.read()
"""


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    free_gib: float = 4.0
    seed: int = 0


class Report(msgspec.Struct):
    passed: bool
    ballast_bytes: int
    card_bytes: int
    #: each stage's facts, as JSON
    recovery: str
    fault: str


def stage(name: str, seed: int, tel: Telemetry) -> dict[str, Any]:
    with tel.stage(name):
        p = subprocess.Popen([sys.executable, "-I", str(CHILD), name, str(seed)], stdout=subprocess.PIPE, text=True)
        result: dict[str, Any] = {"passed": False, "error": "no result"}
        assert p.stdout is not None
        for line in p.stdout:
            try:
                result = json.loads(line)["result"]
            except (ValueError, KeyError, TypeError):
                tel.log(f"{name}: {line.strip()}")
        result["exit"] = p.wait()
        tel.log(f"{name}: {json.dumps(result)}")
        return result


@app.entrypoint
def prove(payload: Probe, tel: Telemetry) -> Report:
    ballast = subprocess.Popen([sys.executable, "-I", "-c", BALLAST, str(int(payload.free_gib * (1 << 30)))],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        assert ballast.stdout is not None
        held, card = (int(v) for v in ballast.stdout.readline().split())
        tel.log(f"ballast holds {held} of {card} bytes")
        seed = payload.seed or time.time_ns() % (1 << 31)
        recovery = stage("recovery", seed, tel)
        fault = stage("fault", seed, tel)
    finally:
        ballast.kill()
        ballast.wait()
    return Report(passed=bool(recovery.get("passed") and fault.get("passed")), ballast_bytes=held, card_bytes=card,
                  recovery=json.dumps(recovery), fault=json.dumps(fault))
