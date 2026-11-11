"""varena's memory-authority rules (`core.py`), each mode in a fresh process: stock Torch's fast path for the cost
comparison, then every case under varena's allocator. One ROW per case (`core-<case>`), and `core` FAIL when a process
exits non-zero (a case's assertion ends it). Run by `release-gate/programs.sh` (xs-174)."""
import json
import subprocess
import sys
from pathlib import Path

CORE = Path(__file__).with_name("core.py")


def run(mode: str, case: str) -> tuple[int, list[dict[str, object]], list[str]]:
    done = subprocess.run([sys.executable, "-I", "-B", str(CORE), mode, case], stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, text=True)
    lines = done.stdout.splitlines()
    return done.returncode, [json.loads(line) for line in lines if line.startswith("{")], lines[-30:]


def row(cell: str, passed: bool, detail: object) -> None:
    print("ROW " + json.dumps({"cell": cell, "result": "PASS" if passed else "FAIL", "detail": detail}, default=str), flush=True)


if __name__ == "__main__":
    code, native, tail = run("native", "fast-path")
    row("core-native-fast-path", code == 0 and bool(native), native or tail)
    code, rows, tail = run("varena", "all")
    for found in rows:
        if "case" in found:
            row(f"core-{found['case']}", True, found)
    if code:
        row("core", False, tail)
