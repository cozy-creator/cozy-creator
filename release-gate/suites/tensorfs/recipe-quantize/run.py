"""Ordinary CLI source→upload→package recipe→encoded model inference. No SDK/rental changes."""

from __future__ import annotations

import argparse
import json
import subprocess
import time
from pathlib import Path


def records(text):
    result, decoder = [], json.JSONDecoder()
    while text.strip():
        value, end = decoder.raw_decode(text.lstrip())
        result.append(value)
        text = text.lstrip()[end:]
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--machine", required=True)
    parser.add_argument(
        "--model",
        required=True,
        help="An owned private org/name for this tiny proof's commits",
    )
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--seed", type=int, default=int(time.time()))
    args = parser.parse_args()
    package = Path(__file__).resolve().parent
    args.out.mkdir(parents=True, exist_ok=True)

    def call(name, argv):
        with (
            (args.out / f"{name}.json").open("w") as stdout,
            (args.out / f"{name}.err").open("w") as stderr,
        ):
            result = subprocess.run(
                ["cozy", *map(str, argv), "--json"],
                stdout=stdout,
                stderr=stderr,
                check=False,
            )
        values = records((args.out / f"{name}.json").read_text())
        assert result.returncode == 0, (name, values[-1] if values else None)
        run = values[-1].get("run", values[-1])
        assert run.get("status") == "completed", (name, run)
        (args.out / f"{name}.number").write_text(str(run["number"]) + "\n")
        return run, values

    made, _ = call(
        "source", ["run", package / "source", f"seed={args.seed}", "--on", args.machine]
    )
    uploaded, _ = call(
        "upload",
        [
            "model",
            "upload",
            f"run:{made['number']}#model",
            args.model,
            "--lane",
            "bf16",
            "--on",
            args.machine,
        ],
    )
    (published,) = [
        m["published"]
        for m in uploaded["result"]["models"]
        if m.get("published", {}).get("destination") == args.model
    ]
    original = args.model + "@" + published["commit"]
    evidence = {
        "source_run": made["number"],
        "upload_run": uploaded["number"],
        "source_commit": original,
        "seed": args.seed,
        "lanes": [],
    }
    for lane, scheme in [("int8", "int8.convrot"), ("nvfp4", "nvfp4")]:
        quantized, events = call(
            "quantize-" + lane,
            [
                "model",
                "quantize",
                original,
                "--lane",
                lane,
                "--package",
                package,
                "--on",
                args.machine,
            ],
        )
        (item,) = [
            o
            for o in quantized["output"]
            if o.get("name") == lane and o.get("checkpoint")
        ]
        checkpoint = item["checkpoint"]
        assert checkpoint["repo_id"] == args.model and checkpoint["lane"] == lane
        committed = checkpoint["repo_id"] + "@" + checkpoint["revision"]
        (measurement,) = [
            v["text"]
            for v in events
            if v.get("type") == "run.log"
            and v.get("text", "").startswith("quantization measurement ")
        ]
        expected_read = 8192 if lane == "int8" else 16384
        expected_new = 4160 if lane == "int8" else 2308
        assert f"source_bytes_read_this_run={expected_read} " in measurement + " "
        assert f"new_bytes_written_this_run={expected_new} " in measurement + " "
        inferred, _ = call(
            "infer-" + lane,
            [
                "run",
                package / "predict",
                f"model.model={committed}",
                f"model.weights={committed}",
                f"seed={args.seed}",
                f"encoding={scheme}",
                "--on",
                args.machine,
                "--gpus=1",
            ],
        )
        assert inferred["result"]["passed"] is True
        evidence["lanes"].append(
            {
                "lane": lane,
                "commit": committed,
                "quantize_run": quantized["number"],
                "inference_run": inferred["number"],
                "measurement": measurement,
                "inference": json.loads(inferred["result"]["detail"]),
            }
        )
    (args.out / "proof.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps(evidence, indent=2))


if __name__ == "__main__":
    main()
