#!/usr/bin/env python3
"""Diagnostic observation of an unchanged real CLI suite; never a substitute pass.

--after is an explicit sampling boundary, not a product execution timeout. If the
suite remains active, collect safe state and finish THIS diagnostic run incomplete.
Other full workflow runs and their processes are untouched.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import time

DIAGNOSTIC_INCOMPLETE = 86


def safe_word(value: object) -> str:
    return "".join(ch for ch in str(value) if ch.isascii() and (ch.isalnum() or ch in "._:-"))[:100]


def proc_sample() -> list[dict]:
    rows = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            if entry.stat().st_uid != os.getuid():
                continue
            comm = (entry / "comm").read_text().strip()
            if not comm.startswith(("cozy", "python", "securefs", "uv", "bash", "sh")):
                continue
            status = (entry / "status").read_text().splitlines()
            parent = next(int(line.split()[1]) for line in status if line.startswith("PPid:"))
            io = {}
            for line in (entry / "io").read_text().splitlines():
                key, value = line.split(":", 1)
                io[safe_word(key)] = int(value.strip())
            tasks = []
            for task in (entry / "task").iterdir():
                try:
                    tasks.append({"tid": int(task.name), "wchan": safe_word((task / "wchan").read_text().strip())})
                except (FileNotFoundError, PermissionError):
                    continue
            rows.append({"pid": int(entry.name), "ppid": parent, "comm": safe_word(comm),
                         "wchan": safe_word((entry / "wchan").read_text().strip()), "io": io, "tasks": tasks})
        except (FileNotFoundError, PermissionError, ProcessLookupError, StopIteration):
            continue
    return rows


def output_holders(paths: list[Path]) -> list[dict]:
    identities = {(path.stat().st_dev, path.stat().st_ino) for path in paths}
    found = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal() or int(entry.name) == os.getpid():
            continue
        try:
            if entry.stat().st_uid != os.getuid():
                continue
            for fd in [1, 2]:
                stat = (entry / "fd" / str(fd)).stat()
                if (stat.st_dev, stat.st_ino) in identities:
                    found.append({"pid": int(entry.name), "comm": safe_word((entry / "comm").read_text().strip()), "stdio_fd": fd})
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            continue
    return found


def probe(cli: Path, *args: str) -> dict:
    try:
        result = subprocess.run([str(cli), "--json", *args], capture_output=True, timeout=5)
        value = json.loads(result.stdout)
        # API summaries can include account URLs, paths or failure text. Retain only
        # execution state and codes, never request bodies or authentication material.
        if isinstance(value, dict) and isinstance(value.get("runs"), list):
            return {"rc": result.returncode, "runs": [
                {key: row[key] for key in ["number", "status", "machine_state"] if key in row}
                | ({"reason_code": safe_word(row.get("reason", {}).get("code", ""))} if row.get("reason") else {})
                for row in value["runs"] if isinstance(row, dict)]}
        return {"rc": result.returncode, "response_keys": sorted(map(safe_word, value)) if isinstance(value, dict) else []}
    except subprocess.TimeoutExpired:
        return {"probe_only": "observation_budget_reached"}
    except (OSError, ValueError):
        return {"probe_only": "unavailable"}


def stage_of(stdout: Path, out: Path) -> tuple[str, bool]:
    with stdout.open("rb") as source:
        source.seek(max(0, stdout.stat().st_size - 65536))
        tail = source.read()
    stages = [
        (b"private execution keeps code", "private-execution"),
        (b"full local rekey", "rekey-preparation"),
        (b"saved recovery and passphrase", "basic-recovery"),
        (b"locking blocks project reads", "basic-lock"),
        (b"encrypt the same project", "initial-encryption"),
        (b"local project encryption", "basic-project-setup"),
        (b"doctor", "startup-doctor"),
        (b"candidates", "startup-candidates"),
        (b"status", "startup-status"),
        (b"install", "startup-install"),
    ]
    for marker, name in stages:
        if marker in tail:
            return name, marker == b"private execution keeps code"
    private = any(out.glob("project-vault.*/private-execution/*.ndjson"))
    return ("private-execution-receipt" if private else "startup-unobserved"), private


def partial_runs(out: Path) -> list[dict]:
    found = []
    for pattern in ["project-vault.*/private-execution/run.ndjson", "project-vault.*/private-execution/failure.ndjson",
                    "project-vault.*/private-alias/remounted-run.ndjson"]:
        for path in out.glob(pattern):
            with path.open("rb") as source:
                source.seek(max(0, path.stat().st_size - 32768))
                lines = source.read().splitlines()
            events = []
            for line in lines:
                try:
                    row = json.loads(line)
                except (ValueError, UnicodeDecodeError):
                    continue
                run = row.get("run", {})
                events.append({"type": safe_word(row.get("type", "")),
                               "status": safe_word(run.get("status", "")),
                               "machine_state": safe_word(run.get("machine_state", "")),
                               "reason_code": safe_word((run.get("reason") or {}).get("code", ""))})
            found.append({"receipt": path.name, "bytes": path.stat().st_size, "events": events[-12:]})
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--after", required=True, type=int)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--fallback", required=True, type=int)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command or not 1 <= args.after <= 600 or not args.after <= args.fallback <= 600:
        parser.error("provide a real suite command and explicit stage/fallback observation bounds")
    args.out.mkdir(parents=True, exist_ok=True)
    stdout = args.out / "diagnostic-suite.stdout.log"
    stderr = args.out / "diagnostic-suite.stderr.log"
    began = time.monotonic()
    with stdout.open("wb") as out, stderr.open("wb") as err:
        process = subprocess.Popen(command, stdout=out, stderr=err, start_new_session=True)
        print("::notice title=vault-diagnostic::diagnostic_only suite observation started", flush=True)
        private_began = None
        stage = "startup-unobserved"
        while process.poll() is None:
            now = time.monotonic()
            stage, private = stage_of(stdout, args.out)
            if private and private_began is None:
                private_began = now
                print("::notice title=vault-diagnostic::private-execution stage observed; explicit sampling window started", flush=True)
            if (private_began is not None and now - private_began >= args.after) or (private_began is None and now - began >= args.fallback):
                break
            time.sleep(1)
        if process.poll() is not None:
            holders = output_holders([stdout, stderr])
            receipt = {"diagnostic_only": bool(holders), "product_complete": not holders,
                       "suite_rc": process.returncode, "stdio_holders_after_suite_exit": holders}
            (args.out / "diagnostic.json").write_text(json.dumps(receipt, indent=2))
            print(json.dumps(receipt), flush=True)
            return DIAGNOSTIC_INCOMPLETE if holders else process.returncode
        # Capture before stopping this separately authorized observation harness.
        receipt = {"diagnostic_only": True, "product_complete": False, "product_result": "incomplete",
                   "observation_boundary_s": args.after, "fallback_boundary_s": args.fallback,
                   "observed_stage": stage, "private_stage_observed": private_began is not None,
                   "poll_rc": process.poll(),
                   "stdout_bytes": stdout.stat().st_size, "stderr_bytes": stderr.stat().st_size,
                   "partial_runs": partial_runs(args.out), "processes": proc_sample(),
                   "status_probe": probe(args.cli, "status"), "runs_probe": probe(args.cli, "run", "ls")}
        receipt["processes_after_probes"] = proc_sample()
        if process.poll() is not None:
            holders = output_holders([stdout, stderr])
            receipt.update(diagnostic_only=bool(holders), product_complete=not holders,
                           product_result="incomplete" if holders else "complete", suite_rc=process.returncode,
                           stdio_holders_after_suite_exit=holders)
            (args.out / "diagnostic.json").write_text(json.dumps(receipt, indent=2))
            print(json.dumps({"product_complete": not holders, "suite_rc": process.returncode}), flush=True)
            return DIAGNOSTIC_INCOMPLETE if holders else process.returncode
        (args.out / "diagnostic.json").write_text(json.dumps(receipt, indent=2))
        print("::notice title=vault-diagnostic::diagnostic_only observation captured; product result incomplete", flush=True)
        # Only this diagnostic suite's process group is stopped. Ordinary full jobs
        # remain running; no product assertion is labelled passed or failed here.
        try:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        except ProcessLookupError:
            pass
    return DIAGNOSTIC_INCOMPLETE


if __name__ == "__main__":
    raise SystemExit(main())
