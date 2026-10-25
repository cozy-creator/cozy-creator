"""Public CPU checkpoint output and persistent replay through the ordinary installed cohort."""

import argparse
import json
import subprocess
import time
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cozy", default="cozy")
    parser.add_argument("--on", default="local")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=False)
    target = Path(__file__).resolve().parent / "derive"
    seed = time.time_ns() & ((1 << 53) - 1)
    report = {"passed": False, "machine": args.on, "seed": seed, "runs": []}
    try:
        for name, recipe in (
            ("cold", "v1"),
            ("warm", "v1"),
            ("changed", "v2"),
            ("reused", "v1"),
        ):
            with (
                (args.out / f"{name}.ndjson").open("w") as log,
                (args.out / f"{name}.stderr").open("w") as errors,
            ):
                process = subprocess.run(
                    [
                        args.cozy,
                        "--json",
                        "run",
                        str(target),
                        "payload:=" + json.dumps({"seed": seed, "recipe": recipe}),
                        "--on",
                        args.on,
                    ],
                    stdout=log,
                    stderr=errors,
                    check=False,
                )
            rows = [
                json.loads(line)
                for line in (args.out / f"{name}.ndjson").read_text().splitlines()
            ]
            terminal = next(
                (row["run"] for row in reversed(rows) if "run" in row), None
            )
            assert terminal is not None, "the CLI did not produce a run receipt"
            report["runs"].append(terminal)
            assert process.returncode == 0 and terminal["status"] == "completed", (
                terminal
            )
            assert terminal["output"], (
                "the checkpoint was not adopted as a real run output"
            )
            result = terminal["result"]
            assert result["exact_bytes"] and result["tree"].startswith("sha256:")
        cold, warm, changed, reused = [run["result"] for run in report["runs"]]
        assert not cold["reused"] and cold["written_bytes"] == 16
        assert not changed["reused"] and changed["written_bytes"] == 16
        for row in (warm, reused):
            assert row["reused"] and row["written_bytes"] == 0
            for key in ("transaction", "checkpoint"):
                assert row["artifact"][key] == cold["artifact"][key]
            assert row["tree"] == cold["tree"] and row["repo_id"] == cold["repo_id"]
        assert changed["artifact"]["transaction"] != cold["artifact"]["transaction"]
        assert changed["repo_id"] != cold["repo_id"]
        report["passed"] = True
    finally:
        (args.out / "receipt.json").write_text(json.dumps(report, indent=2) + "\n")
    print(
        json.dumps({"passed": True, "runs": [run["number"] for run in report["runs"]]})
    )


if __name__ == "__main__":
    main()
