"""Submit real jobs through the default CLI; never install SDKs or operate an unowned job."""

import argparse
import json
import queue
import re
import subprocess
import threading
import time
from datetime import UTC, datetime
from itertools import pairwise
from pathlib import Path


def worker_events(machine, output):
    result = subprocess.run(
        ["cozy", "machine", "logs", machine, "tensord", "--lines", "0", "--json"],
        check=True,
        capture_output=True,
        text=True,
        timeout=60,
    )
    (output / "tensord.ndjson").write_text(result.stdout)
    events = []
    for line in result.stdout.splitlines():
        text = json.loads(line).get("text", "")
        text = re.sub(r"\x1b\[[0-9;]*m", "", text)
        mark = re.search(r"request lookahead (sent|ready|reused|canceled)", text)
        request = re.search(r'\brequest="?([^\s"]+)', text)
        if mark is None or request is None:
            continue
        stamp = text.split(maxsplit=1)[0]
        clock, fraction = (
            stamp.removesuffix("Z").split(".")
            if "." in stamp
            else (stamp.removesuffix("Z"), "")
        )
        seconds = int(datetime.fromisoformat(clock).replace(tzinfo=UTC).timestamp())
        events.append(
            {
                "event": mark[1],
                "request": request[1],
                "at_ns": seconds * 10**9 + int(fraction[:9].ljust(9, "0")),
                "text": text,
            }
        )
    return events


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
    setup_started = time.monotonic()
    deadline = setup_started + 900
    observing = "package setup and first handler entry"

    def wait(condition):
        while not condition():
            if time.monotonic() > deadline:
                raise TimeoutError(
                    f"owned queue gate exceeded its {observing} deadline"
                )
            for run in runs:
                run.consume()

    try:
        a = Run(target / "gpu", args.on, args.out, "A", args.seed, hold_ms=5000)
        runs.append(a)
        wait(lambda: a.entered is not None or a.terminal())
        assert a.entered is not None, a.record
        setup_seconds = time.monotonic() - setup_started
        deadline = time.monotonic() + 180
        observing = "queue handoff observation"
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
        events = worker_events(args.on, args.out)
        relevant = [
            event
            for event in events
            if event["request"] in {run.record["id"] for run in runs}
        ]

        def marks(run, kind):
            return [
                event
                for event in relevant
                if event["request"] == run.record["id"] and event["event"] == kind
            ]

        if args.mode != "incompatible":
            assert marks(b, "sent"), (
                "B never received lookahead; the race window was missed"
            )
        if args.mode in {"handoff", "overlap", "refuse"}:
            assert marks(b, "reused"), (
                "B repeated ordinary preparation instead of reusing lookahead"
            )
        if args.mode == "overlap":
            assert any(
                event["at_ns"] < a.record["result"]["completed_ns"]
                for event in marks(b, "ready")
            ), "B preparation did not finish while A was active"
        if args.mode in {"cancel", "abandon"}:
            assert marks(b, "canceled"), (
                "queued cancellation missed its actual preparation"
            )
        if args.mode == "handoff" and marks(c, "sent"):
            assert marks(c, "reused"), (
                "C discarded an offered preparation during handoff"
            )
        if args.mode == "incompatible":
            assert not any(
                event["at_ns"] < b.record["result"]["completed_ns"]
                for event in marks(c, "sent")
            ), "C lookahead bypassed incompatible B"
        report = {
            "mode": args.mode,
            "runs": [run.record for run in runs],
            "setup_seconds": setup_seconds,
            "handler_order_passed": True,
            "lookahead_events": relevant,
            "lookahead_timing_proof": "Verified actual worker diagnostics, correlated by request ID and worker clock.",
        }
        (args.out / "receipt.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report))
    finally:
        for run in runs:
            if not run.terminal():
                run.cancel()


if __name__ == "__main__":
    main()
