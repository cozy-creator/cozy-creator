"""Real two-request queued GPU preload; uses the existing public-CLI queue controller."""

import argparse
import json
import subprocess
import time
from pathlib import Path

from run import Run, worker_events


def documents(text):
    decoder, position = json.JSONDecoder(), 0
    while position < len(text):
        while position < len(text) and text[position].isspace():
            position += 1
        if position == len(text):
            return
        row, position = decoder.raw_decode(text, position)
        yield row


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--on", required=True)
    parser.add_argument("--model", required=True, help="Owned private proof-model repository")
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--seed", required=True, type=int)
    parser.add_argument("--program", choices=("repeat", "sequence"), default="repeat")
    parser.add_argument(
        "--mode", choices=("overlap", "capacity", "cancel", "control"), default="overlap"
    )
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=False)
    package = Path(__file__).resolve().parent
    runs = []

    def call(label, argv):
        with (
            (args.out / f"{label}.json").open("w") as out,
            (args.out / f"{label}.err").open("w") as err,
        ):
            result = subprocess.run(
                ["cozy", *map(str, argv), "--json"], stdout=out, stderr=err, check=False
            )
        rows = list(documents((args.out / f"{label}.json").read_text()))
        records = [row["run"] for row in rows if isinstance(row.get("run"), dict)]
        record = records[-1] if records else rows[-1]
        assert result.returncode == 0 and record.get("status") == "completed", (label, record)
        return record

    def wait(condition, deadline):
        while not condition():
            assert time.monotonic() < deadline, "owned queue gate missed its observation window"
            for run in runs:
                run.consume()

    def preload_started():
        path = args.out / "A.ndjson"
        if not path.exists():
            return False
        # The CLI reader may be midway through the final NDJSON record. A complete
        # malformed line remains an error; only the unfinished tail is ignored.
        for line in path.read_bytes().splitlines(keepends=True):
            if not line.endswith(b"\n"):
                break
            row = json.loads(line)
            if row.get("type") != "run.log":
                continue
            try:
                text = json.loads(row.get("text", ""))
            except ValueError:
                continue
            if isinstance(text, dict) and text.get("queue_gpu_preload") == "started":
                return text
        return False

    try:
        source = call(
            "source", ["run", package / "preload_source", f"seed={args.seed}", "--on", args.on]
        )
        upload = call(
            "upload",
            [
                "model",
                "upload",
                f"run:{source['number']}#model",
                args.model,
                "--lane",
                "bf16",
                "--on",
                args.on,
            ],
        )
        (published,) = [
            row["published"]
            for row in upload["result"]["models"]
            if row.get("published", {}).get("destination") == args.model
        ]
        model = args.model + "@" + published["commit"]
        values = {"model.model": model, "mode": args.mode, "program": args.program}
        a = Run(package / "preload_gpu", args.on, args.out, "A", args.seed + 1, **values)
        runs.append(a)
        deadline = time.monotonic() + 900
        wait(lambda: a.entered is not None or a.terminal(), deadline)
        assert a.entered is not None and not a.terminal(), a.record
        if args.mode == "control":
            deadline = time.monotonic() + 180
            wait(lambda: a.terminal(), deadline)
            assert a.record["status"] == "completed" and a.record["result"]["passed"]
            proof = {
                "mode": "control",
                "A": {
                    "run": a.record["number"],
                    "detail": json.loads(a.record["result"]["detail"]),
                },
                "scope": "actual no-queued-work native GPU-time control; compare matched repeated pairs separately",
            }
            (args.out / "proof.json").write_text(json.dumps(proof, indent=2) + "\n")
            print(json.dumps(proof, indent=2))
            return
        b = Run(
            package / "preload_gpu",
            args.on,
            args.out,
            "B",
            args.seed + 2,
            rounds=2_000_000,
            **values,
        )
        runs.append(b)
        c = None
        deadline = time.monotonic() + 180
        if args.mode == "cancel":
            wait(lambda: preload_started() or a.terminal(), deadline)
            assert not a.terminal() and preload_started(), "preload never began during A"
            wait(lambda: b.record is not None, deadline)
            b.cancel()
            assert not a.terminal(), "B cancel missed the active native request"
            c_values = {**values, "mode": "capacity"}
            c = Run(package / "preload_gpu", args.on, args.out, "C", args.seed + 3, **c_values)
            runs.append(c)
        wait(lambda: all(run.terminal() for run in runs), deadline)
        for run in runs:
            run.process.wait(timeout=15)
            run.reader.join(timeout=15)
            while not run.events.empty():
                run.consume()
        assert a.record["status"] == "completed" and a.record["result"]["passed"]
        expected = "canceled" if args.mode == "cancel" else "completed"
        assert b.record["status"] == expected, b.record
        events = worker_events(args.on, args.out)
        a_id, b_id = a.request_id(events), b.request_id(events)
        before = json.loads(a.record["result"]["detail"])
        assert before["queued_request"] == b_id
        assert before["cpu_ready_observed_ns"] > before["native_submitted_ns"]
        assert a.entered["native_submitted_ns"] == before["native_submitted_ns"]
        assert before["progress_after_park"] > before["progress_before_park"]
        assert before["evictions_before"] == before["evictions_after"]
        if args.mode == "cancel":
            assert b.entered is None, "canceled queued B entered its handler"
            assert c is not None and c.record["status"] == "completed"
            assert c.record["result"]["passed"]
            successor = json.loads(c.record["result"]["detail"])
            assert successor["demand_bytes"] == 64 << 20
            assert c.record["result"]["pid"] != a.record["result"]["pid"]
        after = None
        if expected == "completed":
            assert b.record["result"]["passed"]
            assert a.record["result"]["pid"] == b.record["result"]["pid"]
            after = json.loads(b.record["result"]["detail"])
            assert before["checkpoint"] == after["checkpoint"]
        proof = {
            "mode": args.mode,
            "source": source["number"],
            "model": model,
            "A": {"run": a.record["number"], "request": a_id, "detail": before},
            "B": {"run": b.record["number"], "request": b_id, "detail": after, "status": expected},
            "C": None if c is None else {"run": c.record["number"], "detail": successor},
            "scope": "Real late CPU-ready queued raw preload/reuse or capacity/cancel correctness; no whole-model preload or general speed claim.",
        }
        (args.out / "proof.json").write_text(json.dumps(proof, indent=2) + "\n")
        print(json.dumps(proof, indent=2))
    finally:
        # Cancel only runs this controller actually submitted; observing timeout is not completion.
        for run in runs:
            if not run.terminal():
                run.cancel()
                end = time.monotonic() + 120
                while not run.terminal() and time.monotonic() < end:
                    run.consume()
                assert run.terminal(), "owned request cancellation did not settle"


if __name__ == "__main__":
    main()
