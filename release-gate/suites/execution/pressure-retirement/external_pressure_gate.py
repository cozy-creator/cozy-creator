"""Local default-CLI pressure proof with one controller-owned foreign CUDA process."""

import argparse
import json
import os
import queue
import select
import subprocess
import threading
import time
from pathlib import Path

from pressure_evidence import documents, memory_notes, qualify


def birth(pid):
    # The parenthesized Linux comm field may contain spaces.
    return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--foreign-python", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--max-foreign-mib", required=True, type=int)
    parser.add_argument("--expected-tensord", required=True)
    parser.add_argument("--seed", required=True, type=int)
    args = parser.parse_args()
    if not hasattr(os, "pidfd_open"):
        parser.error(
            "this exact-exit controller requires Linux pidfds and runs on local"
        )
    if not args.foreign_python.is_file() or not 512 <= args.max_foreign_mib <= 16384:
        parser.error(
            "name an existing Torch interpreter and a foreign bound of512..16384 MiB"
        )
    output = args.output.absolute()
    output.mkdir(parents=True, exist_ok=False)
    root = Path(__file__).resolve().parent
    cozy = str(Path.home() / ".cozy/bin/cozy")
    events, processes = queue.Queue(), {}
    receipt = {
        "began_ns": time.time_ns(),
        "controller_pid": os.getpid(),
        "seed": args.seed,
    }
    run_id, terminal, pidfd = None, None, None
    manual_cancel = False

    def cli(name, *arguments, allow_canceled=False, timeout=120):
        result = subprocess.run(
            [cozy, "--json", *arguments],
            text=True,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
        (output / f"{name}.ndjson").write_text(result.stdout)
        (output / f"{name}.stderr").write_text(result.stderr)
        rows = list(documents(result.stdout))
        final = rows[-1].get("run", rows[-1]) if rows else {}
        if not allow_canceled or final.get("status") != "canceled":
            result.check_returncode()
        return result.stdout, final

    _, machine = cli("machine-before", "machine", "show", "local")
    assert machine["state"] == "ready" and not machine["status"].get("runs"), machine
    assert len(machine["status"].get("gpus", [])) == 1, (
        "this local gate requires exactly one visible GPU"
    )
    assert machine["status"]["commit"] == args.expected_tensord, machine["status"]
    receipt["machine"] = {
        key: machine["status"][key]
        for key in ("commit", "runtime", "tensorfs", "started_at_ms")
    }

    def start(name, command):
        process = subprocess.Popen(
            command,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        processes[name] = process

        def read():
            with (output / f"{name}.ndjson").open("w") as log:
                for line in process.stdout:
                    log.write(line)
                    log.flush()
                    try:
                        row = json.loads(line)
                        if isinstance(row, dict):
                            events.put((name, row))
                    except ValueError:
                        pass
            events.put((name, {"process_exit": process.wait()}))

        threading.Thread(target=read, daemon=True).start()
        return process

    try:
        # Installation has its own bound. No GPU observation deadline is consumed by it.
        cli(
            "installed",
            "run",
            str(root / "gpu"),
            "--warm",
            "installed",
            "--on",
            "local",
            "--gpus=1",
            timeout=900,
        )
        start(
            "managed",
            [
                cozy,
                "--json",
                "run",
                str(root / "gpu"),
                f"seed={args.seed}",
                "--on",
                "local",
                "--gpus=1",
            ],
        )
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
            if (
                pidfd is not None
                and "executor_exit" not in receipt
                and select.select([pidfd], [], [], 0)[0]
            ):
                receipt["executor_exit"] = {
                    "pidfd_readable": True,
                    "at_ns": time.time_ns(),
                }
            try:
                name, row = events.get(timeout=0.1)
            except queue.Empty:
                continue
            if name == "foreign":
                for key in ("foreign_ready", "foreign_result"):
                    if row.get(key):
                        receipt[key] = row
                if "process_exit" in row:
                    receipt["foreign_exit"] = row["process_exit"]
            else:
                record = row.get("run")
                if record is None and "id" in row and "status" in row:
                    record = row
                if isinstance(record, dict):
                    run_id = record["id"]
                    receipt.update(run_id=run_id, run_number=record.get("number"))
                    if record["status"] in ("completed", "failed", "canceled"):
                        terminal = receipt["terminal"] = record
                if row.get("type") == "run.log":
                    try:
                        detail = json.loads(row.get("text", ""))
                    except ValueError:
                        continue
                    if detail.get("pressure_ready") and "foreign" not in processes:
                        receipt["managed_ready"] = detail
                        pid = detail["pid"]
                        born = birth(pid)
                        pidfd = os.pidfd_open(pid)
                        assert birth(pid) == born, (
                            "executor PID changed while opening its exact handle"
                        )
                        receipt["managed_birth"] = {"pid": pid, "start_ticks": born}
                        # Let the existing monitor establish managed demand before foreign growth.
                        time.sleep(1.1)
                        process = start(
                            "foreign",
                            [
                                str(args.foreign_python.absolute()),
                                "-I",
                                str(root / "foreign_cuda.py"),
                                "--max-mib",
                                str(args.max_foreign_mib),
                            ],
                        )
                        receipt["foreign_pid"] = process.pid
                if "process_exit" in row:
                    receipt["managed_exit"] = row["process_exit"]
                    if terminal is None:
                        raise RuntimeError(
                            "managed CLI ended without a terminal receipt"
                        )
            if (
                terminal is not None
                and "foreign_exit" in receipt
                and "executor_exit" in receipt
            ):
                break
        assert terminal is not None and terminal["status"] == "canceled", terminal
        assert "executor_exit" in receipt, "the original exact process has not exited"
        assert (
            receipt.get("foreign_result", {}).get("passed")
            and receipt["foreign_exit"] == 0
        ), receipt
        # Observe reopening using existing machine logs before submitting the successor.
        for index in range(20):
            log, _ = cli(
                f"memory-after-{index}",
                "machine",
                "logs",
                "local",
                "memory",
                "--lines",
                "1000",
            )
            notes = memory_notes(log, receipt["began_ns"])
            if any(
                row.get("event") == "gpu_pressure" and row.get("blocked") is False
                for row in notes
            ):
                break
            time.sleep(0.5)
        else:
            raise AssertionError("pressure admission did not reopen after foreign exit")
        receipt["monitor"] = [
            row for row in notes if row.get("event", "").startswith("gpu_pressure")
        ]
        _, snapshot = cli(
            "run-show", "run", "show", run_id, "--once", allow_canceled=True
        )
        _, receipt["successor"] = cli(
            "successor",
            "run",
            str(root / "gpu"),
            f"seed={args.seed + 1}",
            "pressure=false",
            "--on",
            "local",
            "--gpus=1",
        )
        receipt.update(qualify(receipt, snapshot, notes))
        _, after = cli("machine-after", "machine", "show", "local")
        assert all(
            after["status"][key] == value for key, value in receipt["machine"].items()
        )
        receipt["passed"] = True
    except BaseException as error:
        receipt["error"] = repr(error)
        raise
    finally:
        if run_id is not None and terminal is None:
            manual_cancel = True
            try:
                cli("cleanup-cancel", "run", "cancel", run_id)
            except (OSError, subprocess.SubprocessError) as error:
                receipt["cleanup_error"] = repr(error)
        for name, process in processes.items():
            try:
                process.wait(timeout=180)
            except subprocess.TimeoutExpired:
                # Only this controller's child, never another agent's process or machine.
                process.terminate()
                process.wait(timeout=30)
                receipt.setdefault("forced_controller_children", []).append(name)
                receipt["passed"] = False
        if pidfd is not None:
            os.close(pidfd)
        receipt["manual_cleanup_cancel"] = manual_cancel
        for log in ("memory", "tensord"):
            try:
                cli(f"{log}-final", "machine", "logs", "local", log, "--lines", "1000")
            except (OSError, subprocess.SubprocessError) as error:
                receipt[f"{log}_error"] = repr(error)
        (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, sort_keys=True), flush=True)


if __name__ == "__main__":
    main()
