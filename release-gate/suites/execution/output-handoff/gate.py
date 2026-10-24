"""Actual CLI GPU work vs CPU encoding; no direct machine HTTP or alternate homes."""

import argparse
import json
import queue
import subprocess
import threading
import time
from pathlib import Path


class Run:
    def __init__(
        self, cli, target, output, tag, seed, image, iterations, size, machine="local"
    ):
        self.cli, self.tag = cli, tag
        self.record = None
        self.marks = {}
        self.phases = {}
        self.events = queue.Queue()
        self.errors = (output / f"{tag}.stderr").open("w")
        self.process = subprocess.Popen(
            [
                cli,
                "--json",
                "run",
                str(target),
                f"tag={tag}",
                f"seed={seed}",
                f"image={'true' if image else 'false'}",
                f"iterations={iterations}",
                f"size={size}",
                "--on",
                machine,
                "--gpus=1",
            ],
            stdout=subprocess.PIPE,
            stderr=self.errors,
            text=True,
        )

        def read():
            with (output / f"{tag}.ndjson").open("w") as log:
                for line in self.process.stdout:
                    log.write(line)
                    log.flush()
                    try:
                        self.events.put(json.loads(line))
                    except ValueError:
                        pass
            self.events.put(None)

        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()

    def consume(self, timeout):
        try:
            row = self.events.get(timeout=max(0.001, timeout))
        except queue.Empty:
            return
        if row is None:
            if self.process.wait() != 0 and not self.terminal():
                raise RuntimeError(f"{self.tag}: CLI exited before a terminal receipt")
            return
        record = row.get("run")
        if record is None and "id" in row and "status" in row:
            record = row
        if isinstance(record, dict):
            self.record = record
        if row.get("type") == "run.log":
            try:
                mark = json.loads(row.get("text", ""))
            except ValueError:
                return
            if (
                isinstance(mark, dict)
                and mark.get("tag") == self.tag
                and "handoff" in mark
            ):
                self.marks[mark["handoff"]] = mark
            if isinstance(mark, dict) and mark.get("phase") in {
                "gpu_returned",
                "output_encoding",
                "output_cancelled",
                "output_finished",
            }:
                key = mark["phase"] + ("." + mark["state"] if "state" in mark else "")
                self.phases[key] = {**mark, "at_ns": int(row["at_ms"]) * 1_000_000}

    def terminal(self):
        return self.record is not None and self.record.get("status") in {
            "completed",
            "failed",
            "canceled",
            "refused",
        }

    def wait_for(self, condition, deadline):
        while not condition():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError(f"{self.tag}: proof observation deadline expired")
            self.consume(min(remaining, 1.0))
            if self.terminal() and not condition():
                raise RuntimeError(
                    f"{self.tag}: ended before the required observation: {self.record}"
                )

    def cancel(self):
        if self.record is not None and not self.terminal():
            return subprocess.run(
                [self.cli, "--json", "run", "cancel", self.record["id"]],
                capture_output=True,
                text=True,
                check=True,
            ).stdout
        return None

    def close(self):
        # Only this controller's CLI watcher is ended here; run cancellation uses the public verb.
        if self.process.poll() is None:
            self.process.terminate()
        self.process.wait()
        self.reader.join()
        self.errors.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cozy", default="cozy")
    parser.add_argument(
        "--on", default="local", help="an existing local or rented machine"
    )
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--size", type=int, default=4096)
    parser.add_argument("--cancel-output", action="store_true")
    parser.add_argument("--expect", choices=("overlap", "serial"), default="overlap")
    args = parser.parse_args()
    if not 512 <= args.size <= 4096:
        parser.error("size must be 512..4096")
    if args.cancel_output and args.expect != "overlap":
        parser.error("cancellation while reusing requires the overlap candidate")
    if args.on != "local" and args.expect == "serial":
        parser.error(
            "the uninstrumented serial baseline requires the local clock domain"
        )
    args.out.mkdir(parents=True, exist_ok=False)
    target = Path(__file__).resolve().parent / "run"
    runs = []
    report = {
        "expect": args.expect,
        "cancel_output": args.cancel_output,
        "passed": False,
        "machine": args.on,
    }
    seed = time.time_ns() & ((1 << 53) - 1)
    deadline = time.monotonic() + 180
    try:
        warm = Run(args.cozy, target, args.out, "warm", seed, False, 1, 512, args.on)
        runs.append(warm)
        warm.wait_for(warm.terminal, deadline)
        assert warm.record["status"] == "completed", warm.record

        first = Run(
            args.cozy, target, args.out, "encode", seed + 1, True, 1, args.size, args.on
        )
        runs.append(first)
        first.wait_for(lambda: "gpu_started" in first.marks, deadline)
        second = Run(
            args.cozy,
            target,
            args.out,
            "next",
            seed + 2,
            False,
            4096 if args.cancel_output else 256,
            512,
            args.on,
        )
        runs.append(second)
        second.wait_for(lambda: "gpu_started" in second.marks, deadline)
        if args.cancel_output:
            # Refresh any already queued terminal report before claiming cancellation was exercised.
            while not first.events.empty():
                first.consume(0.001)
            assert not first.terminal(), (
                "encoding finished before cancellation; use a larger real output"
            )
            report["cancel_response"] = first.cancel()
            report["cancel_returned_ns"] = time.time_ns()
        for run in (first, second):
            run.wait_for(run.terminal, deadline)
        assert second.record["status"] == "completed", second.record
        expected_first = "canceled" if args.cancel_output else "completed"
        assert first.record["status"] == expected_first, first.record
        a_end = first.marks["gpu_finished"]["at_ns"]
        b_start = second.marks["gpu_started"]["at_ns"]
        settled = first.record["settled_at_ms"] * 1_000_000
        assert a_end <= b_start, "independent requests overlapped their GPU ownership"
        overlap = False
        if args.expect == "serial":
            assert b_start >= settled, (a_end, b_start, settled)
        assert (
            first.marks["gpu_started"]["pid"] == second.marks["gpu_started"]["pid"]
        ), (
            "the expected same-executor handoff did not occur",
            first.marks,
            second.marks,
        )
        if not args.cancel_output:
            assert first.record.get("output"), "the real image was not committed"
        if args.expect == "overlap":
            begin = first.phases["output_encoding.begin"]["at_ns"]
            end = first.phases["output_encoding.end"]["at_ns"]
            b_end = second.marks["gpu_finished"]["at_ns"]
            assert max(begin, b_start) < min(end, b_end), (
                "GPU work did not overlap real CPU encoding"
            )
            overlap = True
            report["encoding_overlap_ns"] = min(end, b_end) - max(begin, b_start)
            if args.cancel_output:
                canceled = first.phases["output_cancelled"]["at_ns"]
                assert b_start <= canceled < b_end, (
                    "successor finished before cancel delivery"
                )
        report.update(
            passed=True,
            overlap=overlap,
            first_gpu_finished_ns=a_end,
            second_gpu_started_ns=b_start,
            first_settled_ns=settled,
            phases=first.phases,
        )
    finally:
        for run in runs:
            if not run.terminal():
                try:
                    run.cancel()
                except (subprocess.SubprocessError, OSError) as error:
                    report.setdefault("cleanup_errors", []).append(str(error))
            run.close()
        report["runs"] = [run.record for run in runs]
        (args.out / "receipt.json").write_text(json.dumps(report, indent=2) + "\n")
    print(
        json.dumps(
            {"passed": report["passed"], "runs": [r["number"] for r in report["runs"]]}
        )
    )


if __name__ == "__main__":
    main()
