"""Run actual native memory cells through the default CLI; no SDK or rental mutations."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
CLI = Path.home() / ".cozy/bin/cozy"
TERMINAL = {"completed", "failed", "canceled", "refused"}
PREFIXES = (
    "SEQUENCE_GATE ",
    "REPEAT_GATE ",
    "MODEL_STAGE ",
    "STORED_ROLE_STAGE ",
    "STORED_ROLE_CLOSE ",
    "STAGE_FAILURE ",
    "STAGE_PREFETCH ",
)


def rows(path):
    for line in path.read_text().splitlines():
        try:
            value = json.loads(line)
        except ValueError:
            continue
        if isinstance(value, dict):
            yield value


def run_record(path):
    found = [row["run"] for row in rows(path) if isinstance(row.get("run"), dict)]
    assert found, f"no real run receipt in {path}"
    return found[-1]


def settle_owned(path, directory, machine):
    record = run_record(path)
    subprocess.run(
        [str(CLI), "run", "cancel", record["id"], "-y", "--json"],
        check=False,
        capture_output=True,
        timeout=30,
    )
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        shown = subprocess.run(
            [str(CLI), "run", "show", record["id"], "--once", "--json"],
            check=True,
            capture_output=True,
            text=True,
            timeout=20,
        )
        value = json.loads(shown.stdout)
        (directory / "owned-settlement.json").write_text(
            json.dumps(value, indent=2) + "\n"
        )
        if value.get("status") in TERMINAL:
            return
        time.sleep(1)
    raise RuntimeError(
        f"owned run {record['id']} on {machine} has not settled; do not update its SDK"
    )


def events(path):
    found = []
    for row in rows(path):
        message = row.get("text", "")
        for prefix in PREFIXES:
            if message.startswith(prefix):
                # Expected structured telemetry must be valid; never ignore a truncated event.
                event, _ = json.JSONDecoder().raw_decode(message[len(prefix) :])
                found.append((prefix, event))
                break
    return found


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--on", default="local")
    parser.add_argument(
        "--groups", help="comma-separated entrypoint groups; default all"
    )
    parser.add_argument("--out", type=Path)
    parser.add_argument("--seed", type=int, default=3295001)
    parser.add_argument("--execute", action="store_true")
    args = parser.parse_args()
    cells = json.loads((HERE / "cells.json").read_text())
    if args.groups:
        groups = set(args.groups.split(","))
        unknown = groups - {cell["entrypoint"] for cell in cells}
        assert not unknown, unknown
        cells = [cell for cell in cells if cell["entrypoint"] in groups]
    if not args.execute:
        print(
            json.dumps(
                {"cells": cells, "sdk_mutation": False, "gpu_action": False}, indent=2
            )
        )
        return
    if args.out is None:
        parser.error("--execute requires a fresh --out")
    args.out.mkdir(parents=True, exist_ok=False)
    source = {
        str(path.relative_to(HERE)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in HERE.rglob("*")
        if path.is_file()
        and "__pycache__" not in path.parts
        and ".venv" not in path.parts
        and not path.name.endswith(".pyc")
    }
    (args.out / "source-sha256.json").write_text(json.dumps(source, indent=2) + "\n")
    before = subprocess.run(
        [str(CLI), "machine", "show", args.on, "--json"],
        capture_output=True,
        text=True,
        check=True,
        timeout=30,
    )
    (args.out / "machine-before.json").write_text(before.stdout)
    results, previous_pid = [], None
    for index, cell in enumerate(cells):
        path = args.out / (cell["name"] + ".ndjson")
        params = {"seed": args.seed + index, **cell["args"]}
        argv = [str(CLI), "run", str(HERE / cell["entrypoint"])]
        argv += [
            f"{key}={str(value).lower() if isinstance(value, bool) else value}"
            for key, value in params.items()
        ]
        argv += ["--on", args.on, "--gpus=1", "--json"]
        (args.out / (cell["name"] + "-command.json")).write_text(
            json.dumps(argv, indent=2) + "\n"
        )
        try:
            with path.open("x") as stream:
                process = subprocess.run(
                    argv,
                    check=False,
                    stdout=stream,
                    stderr=subprocess.STDOUT,
                    timeout=360,
                )
        except subprocess.TimeoutExpired:
            settle_owned(path, args.out, args.on)
            raise
        record = run_record(path)
        if record.get("status") not in TERMINAL:
            settle_owned(path, args.out, args.on)
            raise RuntimeError("watcher ended before owned execution was terminal")
        observed = events(path)
        detail = None
        pid = next((event["pid"] for _, event in observed if "pid" in event), None)
        verdict = {
            "cell": cell,
            "run": record["number"],
            "status": record["status"],
            "reason": record.get("reason"),
            "pid": pid,
            "passed": False,
        }
        results.append(verdict)
        (args.out / "RESULTS.json").write_text(json.dumps(results, indent=2) + "\n")
        if cell["expect"] == "completed":
            assert process.returncode == 0 and record["status"] == "completed", verdict
            assert record["result"]["passed"] is True, verdict
            detail = json.loads(record["result"]["detail"])
            if "pid" in detail:
                pid = detail["pid"]
        else:
            assert process.returncode != 0 and record["status"] == "failed", verdict
            reason = record.get("reason") or {}
            assert reason.get(
                "code"
            ) != "device_fault" and "retire executor" not in str(reason), verdict
            kind = cell["expect"].split(":", 1)[1]
            if kind == "stale":
                event = next(e for _, e in observed if e.get("event") == "refused")
                assert event["zero_launches"] and event["next_definition_valid"]
                assert "storage or geometry changed" in str(reason)
            elif kind == "canceled":
                event = next(e for _, e in observed if e.get("event") == "refused")
                assert event["canceled_predecessor"] and event["zero_later_launches"]
                assert "preceding prepared stage was canceled" in str(reason)
            elif kind == "stored-source":
                event = next(
                    e for prefix, e in observed if prefix == "STORED_ROLE_CLOSE "
                )
                assert (
                    event["closed"]
                    and event["cert"]
                    and event["pins"] == 0
                    and event["users"] == 1
                )
            elif kind == "source":
                event = next(
                    e for _, e in observed if e.get("event") == "read-failed-cleanly"
                )
                assert event["completion"] and event["pins"] == 0 and event["no_launch"]
            else:
                raise AssertionError(kind)
        assert pid is not None, "missing exact executor identity"
        if cell["same_pid"]:
            assert pid == previous_pid, (cell["name"], pid, previous_pid)
        previous_pid = pid
        verdict.update(pid=pid, passed=True, detail=detail)
        (args.out / "RESULTS.json").write_text(json.dumps(results, indent=2) + "\n")
        print(
            json.dumps({"cell": cell["name"], "run": record["number"], "passed": True}),
            flush=True,
        )
    after = subprocess.run(
        [str(CLI), "machine", "show", args.on, "--json"],
        capture_output=True,
        text=True,
        check=True,
        timeout=30,
    )
    (args.out / "machine-after.json").write_text(after.stdout)


if __name__ == "__main__":
    main()
