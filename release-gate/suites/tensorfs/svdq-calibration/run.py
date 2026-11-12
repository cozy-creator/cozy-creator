"""Normal CLI calls: the Runtime's Output reuse and resume (bench.quantize_reuse), then source→calibrate/adopt→upload→
held-out model calls, whose predict checks the stored parts through its own grant. No SDK or rental changes."""

from __future__ import annotations

import argparse
import json
import subprocess
import time
from pathlib import Path
from typing import TypedDict

type JSON = str | int | float | bool | None | list[JSON] | dict[str, JSON]


class Published(TypedDict, total=False):
    destination: str
    commit: str


class Uploaded(TypedDict, total=False):
    published: Published


class Result(TypedDict, total=False):
    models: list[Uploaded]
    passed: bool
    detail: str


class Run(TypedDict, total=False):
    number: int
    status: str
    result: Result


class Line(Run, total=False):
    """One `--json` record: a run, bare or under `run`."""

    run: Run


class Called(TypedDict):
    run: int
    detail: JSON


class Args(argparse.Namespace):
    machine: str
    model: str
    out: Path
    seed: int
    require_native: bool


def records(text: str) -> list[Line]:
    result: list[Line] = []
    decoder = json.JSONDecoder()
    while text.strip():
        value, end = decoder.raw_decode(text.lstrip())
        result.append(value)
        text = text.lstrip()[end:]
    return result


def main() -> None:
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
    args = parser.parse_args(namespace=Args())
    package = Path(__file__).resolve().parent
    args.out.mkdir(parents=True, exist_ok=True)

    # A run belongs to a project.
    # cozy 0.24 (xs-174): `cozy new <dir>` makes the project (a project.db), and a run belongs to the project it runs in.
    # cozy 0.24.0's projects are plain and it has no --unencrypted; later ones encrypt unless told not to.
    if not (args.out / "project" / "project.db").is_file():
        plain = ["--unencrypted"] if "--unencrypted" in subprocess.run(["cozy", "new", "--help"], capture_output=True, text=True).stdout else []
        subprocess.run(["cozy", "new", *plain, str(args.out / "project")], check=True, stdout=subprocess.DEVNULL)

    def call(name: str, argv: list[str | Path]) -> Run:
        with (
            (args.out / f"{name}.json").open("w") as stdout,
            (args.out / f"{name}.err").open("w") as stderr,
        ):
            result = subprocess.run(
                ["cozy", "--json", *map(str, argv)],
                cwd=args.out / "project",
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

    def upload(name: str, number: int, slot: str, lane: str) -> tuple[Run, str]:
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

    reuse: dict[str, Called] = {}
    for case in ("components", "resume"):
        done = call(case, ["run", package / case, f"seed={args.seed}", "--on", args.machine])
        assert done["result"]["passed"] is True
        reuse[case] = {"run": done["number"], "detail": json.loads(done["result"]["detail"])}

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
    calibration = json.loads(calibrated["result"]["detail"])
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
            f"conversion={calibration['conversion']}",
            "--on",
            args.machine,
            "--gpus=1",
        ],
    )
    assert predicted["result"]["passed"] is True
    proof = {
        "seed": args.seed,
        "reuse": reuse,
        "source_run": made["number"],
        "upload_source_run": uploaded["number"],
        "source_commit": original,
        "calibration_run": calibrated["number"],
        "calibration": calibration,
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
