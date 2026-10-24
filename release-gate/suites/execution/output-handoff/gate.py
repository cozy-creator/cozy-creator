"""Actual CLI GPU work vs CPU encoding; no direct machine HTTP or alternate homes."""

import argparse
import hashlib
import json
import queue
import subprocess
import threading
import time
from pathlib import Path


class Run:
    def __init__(
        self,
        cli,
        target,
        output,
        tag,
        seed,
        image,
        iterations,
        size,
        machine="local",
        file_mib=0,
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
                f"file_mib={file_mib}",
                "--on",
                machine,
                "--gpus=1",
                "--out",
                str(output / f"{tag}-assets"),
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
                self.phases[key] = {
                    **mark,
                    "at_ns": int(mark.get("at_ns", int(row["at_ms"]) * 1_000_000)),
                }

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

    def verify_image(self):
        from PIL import Image

        outputs = self.record.get("output", [])
        assert len(outputs) == 1, "one returned image was not published"
        path = Path(outputs[0]["path"])
        with Image.open(path) as saved:
            actual = hashlib.sha256(saved.convert("RGB").tobytes()).hexdigest()
        assert actual == self.record["result"]["pixels_sha256"], (
            "the downloaded lossless image differs from the actual GPU pixels"
        )

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
    cancellation = parser.add_mutually_exclusive_group()
    cancellation.add_argument("--cancel-output", action="store_true")
    cancellation.add_argument("--cancel-custody", action="store_true")
    parser.add_argument("--file-mib", type=int, default=240)
    parser.add_argument("--expect", choices=("overlap", "baseline"), default="overlap")
    args = parser.parse_args()
    if not 512 <= args.size <= 4096:
        parser.error("size must be 512..4096")
    cancel = args.cancel_output or args.cancel_custody
    if cancel and args.expect != "overlap":
        parser.error("cancellation while reusing requires the overlap candidate")
    if not 1 <= args.file_mib <= 240:
        parser.error("file-mib must be 1..240")
    args.out.mkdir(parents=True, exist_ok=False)
    target = Path(__file__).resolve().parent / "run"
    runs = []
    report = {
        "expect": args.expect,
        "cancel_output": args.cancel_output,
        "cancel_custody": args.cancel_custody,
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
            args.cozy,
            target,
            args.out,
            "encode",
            seed + 1,
            not args.cancel_custody,
            1,
            args.size,
            args.on,
            args.file_mib if args.cancel_custody else 0,
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
            8192 if args.cancel_custody else 4096 if args.cancel_output else 256,
            512,
            args.on,
        )
        runs.append(second)
        second.wait_for(lambda: "gpu_started" in second.marks, deadline)
        if args.cancel_custody:
            first.wait_for(lambda: "output_encoding.end" in first.phases, deadline)
        if cancel:
            # Refresh any already queued terminal report before claiming cancellation was exercised.
            while not first.events.empty():
                first.consume(0.001)
            assert not first.terminal(), (
                "output settled before cancellation; this run did not exercise the race"
            )
            report["cancel_response"] = first.cancel()
            report["cancel_returned_ns"] = time.time_ns()
        for run in (first, second):
            run.wait_for(run.terminal, deadline)
        assert second.record["status"] == "completed", second.record
        expected_first = "canceled" if cancel else "completed"
        assert first.record["status"] == expected_first, first.record
        a_end = first.marks["gpu_finished"]["at_ns"]
        b_start = second.marks["gpu_started"]["at_ns"]
        settled = first.record["settled_at_ms"] * 1_000_000
        assert a_end <= b_start, "independent requests overlapped their GPU ownership"
        overlap = (
            None  # An uninstrumented baseline does not identify the encoding interval.
        )
        assert (
            first.marks["gpu_started"]["pid"] == second.marks["gpu_started"]["pid"]
        ), (
            "the expected same-executor handoff did not occur",
            first.marks,
            second.marks,
        )
        if not cancel:
            first.verify_image()
        if args.expect == "overlap":
            begin = first.phases[
                "output_encoding.end"
                if args.cancel_custody
                else "output_encoding.begin"
            ]["at_ns"]
            end = first.phases[
                "output_finished" if args.cancel_custody else "output_encoding.end"
            ]["at_ns"]
            b_end = second.marks["gpu_finished"]["at_ns"]
            assert max(begin, b_start) < min(end, b_end), (
                "GPU work did not overlap the selected real CPU output interval"
            )
            overlap = True
            report["output_overlap_ns"] = min(end, b_end) - max(begin, b_start)
            if cancel:
                canceled = first.phases["output_cancelled"]["at_ns"]
                assert b_start <= canceled < b_end, (
                    "successor finished before cancel delivery"
                )
                if args.cancel_custody:
                    assert begin <= canceled < end, (
                        "cancel missed the real custody interval"
                    )
        report.update(
            passed=True,
            overlap=overlap,
            first_gpu_finished_ns=a_end,
            second_gpu_started_ns=b_start,
            gpu_handoff_gap_ns=b_start - a_end,
            first_controller_settled_ns=settled,
            candidate_qualified=args.expect == "overlap",
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
