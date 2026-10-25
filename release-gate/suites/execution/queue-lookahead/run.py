"""Submit real jobs through the default CLI; never install SDKs or operate an unowned job."""

import argparse
import json
import queue
import subprocess
import threading
import time
from itertools import pairwise
from pathlib import Path


class Run:
    def __init__(self, target, machine, output, tag, seed, **values):
        self.tag, self.record, self.entered = tag, None, None
        self.events = queue.Queue()
        self.stderr = (output / f"{tag}.stderr").open("w")
        self.process = subprocess.Popen(
            [
                "cozy",
                "--json",
                "run",
                str(target),
                f"tag={tag}",
                f"seed={seed}",
                *(f"{key}={str(value).lower()}" for key, value in values.items()),
                "--on",
                machine,
                "--gpus=1",
            ],
            stdout=subprocess.PIPE,
            stderr=self.stderr,
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

    def consume(self):
        try:
            row = self.events.get(timeout=0.1)
        except queue.Empty:
            return
        if row is None:
            if not self.terminal():
                raise RuntimeError(f"{self.tag}: CLI ended without a terminal receipt")
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
            if isinstance(mark, dict) and mark.get("queue_gate") == "entered":
                self.entered = mark

    def terminal(self):
        return self.record is not None and self.record.get("status") in {
            "completed",
            "failed",
            "canceled",
            "refused",
        }

    def cancel(self):
        if self.record is not None and not self.terminal():
            subprocess.run(
                ["cozy", "--json", "run", "cancel", str(self.record["id"])],
                check=True,
                capture_output=True,
                text=True,
            )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--on", default="local")
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--seed", required=True, type=int)
    parser.add_argument(
        "--mode",
        choices=(
            "handoff",
            "overlap",
            "cancel",
            "abandon",
            "refuse",
            "cancel-active",
            "incompatible",
        ),
        default="handoff",
    )
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=False)
    target = Path(__file__).parent
    runs = []
    deadline = time.monotonic() + 180

    def wait(condition):
        while not condition():
            if time.monotonic() > deadline:
                raise TimeoutError("owned queue gate exceeded its observation deadline")
            for run in runs:
                run.consume()

    try:
        a = Run(target / "gpu", args.on, args.out, "A", args.seed, hold_ms=5000)
        runs.append(a)
        wait(lambda: a.entered is not None or a.terminal())
        assert a.entered is not None, a.record
        rounds = {
            "overlap": 3_000_000,
            "cancel": 10_000_000,
            "abandon": 50_000_000,
        }.get(args.mode, 1)
        b = Run(
            target / ("other" if args.mode == "incompatible" else "gpu"),
            args.on,
            args.out,
            "B",
            args.seed + 1,
            rounds=rounds,
            reject=args.mode == "refuse",
        )
        runs.append(b)
        wait(lambda: b.record is not None)
        if args.mode in {"cancel", "abandon", "cancel-active"}:
            observed = time.monotonic()
            wait(lambda: time.monotonic() - observed >= 0.3)
            (a if args.mode == "cancel-active" else b).cancel()
        c = Run(target / "gpu", args.on, args.out, "C", args.seed + 2)
        runs.append(c)
        wait(lambda: all(run.terminal() for run in runs))
        for run in runs:
            run.process.wait(timeout=15)
            run.reader.join(timeout=15)
            while not run.events.empty():
                run.consume()
        expected = [
            "canceled" if args.mode == "cancel-active" else "completed",
            "canceled"
            if args.mode in {"cancel", "abandon"}
            else "failed"
            if args.mode == "refuse"
            else "completed",
            "completed",
        ]
        for run, status in zip(runs, expected, strict=True):
            assert run.record["status"] == status, run.record
        successful = [
            run.record["result"] for run in runs if run.record["status"] == "completed"
        ]
        for previous, following in pairwise(successful):
            assert following["entered_ns"] >= previous["completed_ns"], (
                previous,
                following,
            )
        if args.mode in {"handoff", "overlap", "refuse"}:
            assert len({result["pid"] for result in successful}) == 1, successful
        if args.mode == "cancel-active":
            assert b.record["result"]["pid"] != a.entered["pid"]
        if args.mode in {"cancel", "abandon", "refuse"}:
            assert b.entered is None, "a queued/refused job entered its handler"
        if args.mode in {"cancel", "abandon"}:
            assert c.record["result"]["pid"] != a.record["result"]["pid"], (
                "revoked preparation did not retire after the active call",
                a.record,
                c.record,
            )
        report = {
            "mode": args.mode,
            "runs": [run.record for run in runs],
            "handler_order_passed": True,
            "lookahead_timing_proof": "Requires the matching TensorD sent/ready/reused diagnostics; completion alone is insufficient.",
        }
        (args.out / "receipt.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report))
    finally:
        for run in runs:
            if not run.terminal():
                run.cancel()


if __name__ == "__main__":
    main()
