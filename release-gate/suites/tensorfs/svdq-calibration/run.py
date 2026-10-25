"""Normal CLI source→calibrate/adopt→upload→held-out model calls; no SDK/rental changes."""

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
        "--model", required=True, help="An owned private model repository"
    )
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--seed", type=int, default=int(time.time()))
    parser.add_argument(
        "--require-native", action="store_true", help="Require SM100+ encoded SVDQ GEMM"
    )
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
        return run

    def upload(name, number, slot, lane):
        result = call(
            name,
            [
                "model",
                "upload",
                f"run:{number}#{slot}",
                args.model,
                "--lane",
                lane,
                "--on",
                args.machine,
            ],
        )
        (published,) = [
            row["published"]
            for row in result["result"]["models"]
            if row.get("published", {}).get("destination") == args.model
        ]
        return result, args.model + "@" + published["commit"]

    made = call(
        "source", ["run", package / "source", f"seed={args.seed}", "--on", args.machine]
    )
    uploaded, original = upload("upload-source", made["number"], "model", "bf16")
    calibrated = call(
        "calibrate",
        [
            "run",
            package / "calibrate",
            f"model.model={original}",
            f"model.weights={original}",
            f"seed={args.seed}",
            "--on",
            args.machine,
            "--gpus=1",
        ],
    )
    assert calibrated["result"]["passed"] is True
    converted, encoded = upload(
        "upload-svdq", calibrated["number"], "svdq", "svdq-nvfp4"
    )
    predicted = call(
        "predict",
        [
            "run",
            package / "predict",
            f"model.model={encoded}",
            f"model.weights={encoded}",
            f"seed={args.seed}",
            f"require_native={'true' if args.require_native else 'false'}",
            "--on",
            args.machine,
            "--gpus=1",
        ],
    )
    assert predicted["result"]["passed"] is True
    proof = {
        "seed": args.seed,
        "source_run": made["number"],
        "upload_source_run": uploaded["number"],
        "source_commit": original,
        "calibration_run": calibrated["number"],
        "calibration": json.loads(calibrated["result"]["detail"]),
        "upload_svdq_run": converted["number"],
        "encoded_commit": encoded,
        "prediction_run": predicted["number"],
        "prediction": json.loads(predicted["result"]["detail"]),
        "scope": "real SVDQ component/calibration correctness; not full-model performance",
    }
    (args.out / "proof.json").write_text(json.dumps(proof, indent=2) + "\n")
    print(json.dumps(proof, indent=2))


if __name__ == "__main__":
    main()
