"""Real output backlog: bounded detached CPU work while one GPU serves queued requests."""

import argparse
import json
import subprocess
import time
from itertools import pairwise
from pathlib import Path

from gate import Run


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cozy", default="cozy")
    parser.add_argument(
        "--on", default="local", help="an existing local or rented machine"
    )
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument(
        "--cpu-capacity",
        type=int,
        required=True,
        help="the candidate service's actual configured parallelism",
    )
    parser.add_argument("--count", type=int, default=0)
    parser.add_argument("--size", type=int, default=4096)
    args = parser.parse_args()
    count = args.count or args.cpu_capacity + 3
    if not 1 <= args.cpu_capacity <= 8 or not args.cpu_capacity + 2 <= count <= 16:
        parser.error("capacity must be 1..8, with capacity+2 <= count <= 16")
    if not 512 <= args.size <= 4096:
        parser.error("size must be 512..4096")
    args.out.mkdir(parents=True, exist_ok=False)
    target = Path(__file__).resolve().parent / "run"
    deadline = time.monotonic() + 300
    seed = time.time_ns() & ((1 << 53) - 1)
    runs = []
    report = {
        "passed": False,
        "cpu_capacity": args.cpu_capacity,
        "gpu_slots": 1,
        "machine": args.on,
    }
    try:
        warm = Run(args.cozy, target, args.out, "warm", seed, False, 1, 512, args.on)
        runs.append(warm)
        warm.wait_for(warm.terminal, deadline)
        assert warm.record["status"] == "completed", warm.record
        for index in range(count):
            runs.append(
                Run(
                    args.cozy,
                    target,
                    args.out,
                    f"burst-{index}",
                    seed + index + 1,
                    True,
                    1,
                    args.size,
                    args.on,
                )
            )
        for run in runs[1:]:
            run.wait_for(run.terminal, deadline)
            assert run.record["status"] == "completed", run.record
            assert run.record.get("output"), "a real image was not committed"
        intervals, events = [], []
        for run in runs[1:]:
            begin = run.phases["gpu_returned"]["at_ns"]
            end = run.phases["output_finished"]["at_ns"]
            assert begin < end, "output interval has insufficient timestamp resolution"
            events.extend(((begin, 1), (end, -1)))
            intervals.append(
                (run.marks["gpu_started"]["at_ns"], run.marks["gpu_finished"]["at_ns"])
            )
        active = peak = 0
        # Conservative at millisecond resolution: simultaneous starts precede finishes.
        for _, change in sorted(events, key=lambda event: (event[0], -event[1])):
            active += change
            assert active >= 0
            peak = max(peak, active)
        assert active == 0
        assert peak <= args.cpu_capacity + 1, (
            "detached output work exceeded capacity plus one GPU slot"
        )
        assert peak == args.cpu_capacity + 1, (
            "the burst did not exercise output backpressure"
        )
        intervals.sort()
        assert all(a[1] <= b[0] for a, b in pairwise(intervals)), "GPU calls overlapped"
        pids = {run.marks["gpu_started"]["pid"] for run in runs[1:]}
        assert len(pids) == 1, "same-construction burst did not reuse one executor"
        report.update(
            passed=True,
            peak_detached_outputs=peak,
            gpu_intervals=intervals,
            pid=next(iter(pids)),
            count=count,
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
            {"passed": report["passed"], "peak": report["peak_detached_outputs"]}
        )
    )


if __name__ == "__main__":
    main()
