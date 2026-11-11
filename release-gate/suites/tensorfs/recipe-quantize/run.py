"""Ordinary CLI source→upload→package recipe→encoded model inference. No SDK/rental changes."""

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


class Committed(TypedDict):
    repo_id: str
    lane: str
    revision: str


class Output(TypedDict, total=False):
    name: str
    checkpoint: Committed


class Result(TypedDict, total=False):
    models: list[Uploaded]
    passed: bool
    detail: str


class Run(TypedDict, total=False):
    number: int
    status: str
    result: Result
    output: list[Output]


class Line(Run, total=False):
    """One `--json` record: a run (bare or under `run`) or one of its events."""

    run: Run
    type: str
    text: str


class Lane(TypedDict):
    lane: str
    commit: str
    quantize_run: int
    inference_run: int
    measurement: str
    inference: JSON


class Evidence(TypedDict):
    source_run: int
    upload_run: int
    source_commit: str
    seed: int
    lanes: list[Lane]


class Args(argparse.Namespace):
    machine: str
    model: str
    out: Path
    seed: int


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
        "--model",
        required=True,
        help="An owned private org/name for this tiny proof's commits",
    )
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--seed", type=int, default=int(time.time()))
    args = parser.parse_args(namespace=Args())
    package = Path(__file__).resolve().parent
    args.out.mkdir(parents=True, exist_ok=True)

    # A run belongs to a project.
    # cozy 0.24 (xs-174): `cozy new <dir>` makes the project (a project.db), and a run belongs to the project it runs in.
    if not any((args.out / "project" / name).is_file() for name in ("project.db", "project.yaml")):
        subprocess.run(["cozy", "new", str(args.out / "project")], check=True, stdout=subprocess.DEVNULL)

    def call(name: str, argv: list[str | Path]) -> tuple[Run, list[Line]]:
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
    evidence: Evidence = {
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
