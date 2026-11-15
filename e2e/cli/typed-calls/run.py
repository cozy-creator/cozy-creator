# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow>=12"]
# ///

"""Run only public Cozy CLI requests, in an existing project and the default home.

The coordinator installs the combined candidate first. This script never changes services,
SDK policy, machine roots, or environment variables. Each invocation uses a fresh receipt
directory so earlier failures stay available.
"""

import argparse
import json
import os
import re
import struct
import subprocess
from collections.abc import Sequence
from pathlib import Path
from typing import TypedDict, cast

type Json = None | bool | int | float | str | list[Json] | dict[str, Json]
type JsonObject = dict[str, Json]


class CheckedRun(TypedDict):
    run: JsonObject
    watched: JsonObject
    result: JsonObject


def as_object(value: object) -> JsonObject:
    assert isinstance(value, dict) and all(isinstance(key, str) for key in value)
    return cast(JsonObject, value)


def as_text(value: Json) -> str:
    assert isinstance(value, str), value
    return value


def main() -> None:
    parser = argparse.ArgumentParser()
    binary = "cozy.exe" if os.name == "nt" else "cozy"
    parser.add_argument("--cozy", default=str(Path.home() / ".cozy" / "bin" / binary))
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--fixture", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--model-prefix", required=True)
    parser.add_argument("--argv-log", type=Path, help="append CLI words in the e2e coverage format")
    args = parser.parse_args()
    args.project = args.project.resolve()
    args.fixture = args.fixture.resolve()
    args.out = args.out.resolve()
    if not re.fullmatch(r"[a-z][a-z0-9-]+", args.model_prefix):
        parser.error("model-prefix must be a fresh lowercase local model name prefix")
    args.out.mkdir(parents=True, exist_ok=False)
    commands: list[dict[str, object]] = []

    def cli(label: str, *words: str | Path, success: bool = True) -> list[JsonObject]:
        command = [args.cozy, "--json", *map(str, words)]
        if args.argv_log:
            with args.argv_log.open("a") as log:
                log.write("\x1f".join(command[1:]) + "\x1f\x1e")
        print(json.dumps({"case": label, "command": command}), flush=True)
        with (
            (args.out / (label + ".ndjson")).open("w") as stdout,
            (args.out / (label + ".stderr")).open("w") as stderr,
        ):
            ran = subprocess.run(
                command, cwd=args.project, stdout=stdout, stderr=stderr, check=False
            )
        commands.append(
            {"case": label, "command": command, "exit_code": ran.returncode}
        )
        (args.out / "commands.json").write_text(json.dumps(commands, indent=2) + "\n")
        if success:
            assert ran.returncode == 0, (label, "CLI exit", ran.returncode)
        else:
            assert ran.returncode != 0, (
                label,
                "private request unexpectedly succeeded",
            )
        return [
            as_object(json.loads(line))
            for line in (args.out / (label + ".ndjson")).read_text().splitlines()
            if line.strip()
        ]

    def terminal(rows: Sequence[JsonObject]) -> JsonObject:
        ended = [
            as_object(row["run"])
            for row in rows
            if row.get("type") in ("run.completed", "run.failed", "run.canceled")
        ]
        assert ended, (
            "CLI supplied no terminal run; preserve the receipts and inspect the live run"
        )
        return ended[-1]

    def run(label: str, *inputs: str) -> CheckedRun:
        rows = cli(
            label,
            "run",
            args.fixture / "caller/run",
            *inputs,
            "--on",
            "local",
            "-o",
            args.out / "exports" / label,
        )
        record = terminal(rows)
        assert record["status"] == "completed", (label, record.get("reason"))
        handle = as_text(record.get("name") or str(record["number"]))
        watched = cli(label + "-watch", "run", "watch", handle, "--once", "--full")[-1]
        assert watched["status"] == "completed"
        assert watched["result"] == record["result"], (label, "watch/result mismatch")
        result = as_object(record["result"])
        assert result["child_installed"] is False, (
            label,
            "callee implementation visible in caller",
        )
        assert result["callee_dependency_installed"] is False, (
            label,
            "xxhash visible in caller",
        )
        for field in (
            "shared_torch_available",
            "shared_sympy_available",
            "torch_imported",
        ):
            assert type(result[field]) is bool, (label, field)
        assert result["python"], (label, "missing caller interpreter")
        return {"run": record, "watched": watched, "result": result}

    # Exercise cl-701 through the public CLI before any actual package run.
    cli("describe", "run", args.fixture / "caller/run", "--describe", "--on", "local")

    def check_media(checked: CheckedRun, label: str) -> None:
        result = checked["result"]
        assert result["error"] == ""
        assert result["prime"] is True  # generated request default number=17
        assert result["squared"] == 289  # the real memoized leaf, through its wheel binding
        assert re.fullmatch(r"[0-9a-f]{16}", as_text(result["fingerprint"]))
        assert result["child_python"] and result["python"] != result["child_python"]
        assert result["descendant_python"] == result["child_python"]
        assert ".cozy-calls" in Path(as_text(result["child_python"])).parts
        assert as_object(result["image"])["media_type"] == "image/png"
        pngs = list((args.out / "exports" / label).glob("*.png"))
        assert len(pngs) == 1, pngs
        from PIL import Image

        with Image.open(pngs[0]) as image:
            assert image.format == "PNG" and image.size == (4, 3)
            pixels = image.convert("RGBA")
            assert pixels.getpixel((0, 0)) == (7, 11, 19, 95)
            assert pixels.getpixel((1, 0)) == (15, 63, 127, 255)

    media = run("media")
    check_media(media, "media")
    assets = run("assets", "use_assets=true")
    check_media(assets, "assets")

    failure = run("failure", "fail=true")
    result = failure["result"]
    # A deliberately catches ChildCallError, so its own run completes successfully.
    assert result["error"] == "typed_fixture_failure", result
    assert result["child_request"], result
    assert result["image"] is None

    model_results: list[CheckedRun] = []
    for label, value in (("one", 1.25), ("two", -2.5)):
        filename = args.out / (label + ".safetensors")
        header = json.dumps(
            {"w": {"dtype": "F32", "shape": [4], "data_offsets": [0, 16]}}
        ).encode()
        header += b" " * (-len(header) % 8)
        filename.write_bytes(
            struct.pack("<Q", len(header)) + header + struct.pack("<4f", value, 2, 3, 4)
        )
        reference = "local/" + args.model_prefix + "-" + label
        cli(
            "install-" + label,
            "model",
            "install",
            filename,
            reference,
            "--source-format",
            "as-is",
            "--on",
            "local",
        )
        made = run("model-" + label, "choice=" + reference)
        result = made["result"]
        assert result["value"] == value, result
        assert re.fullmatch(r"sha256:[0-9a-f]{64}", as_text(result["checkpoint"])), (
            result
        )
        assert result["child_python"] and result["python"] != result["child_python"]
        assert ".cozy-calls" in Path(as_text(result["child_python"])).parts
        model_results.append(made)
    assert (
        model_results[0]["result"]["checkpoint"]
        != model_results[1]["result"]["checkpoint"]
    )

    private = cli(
        "private",
        "run",
        args.fixture / "leaf/private",
        "number=17",
        "--on",
        "local",
        success=False,
    )
    assert any(
        as_object(row.get("error", {})).get("code") == "callable_internal"
        for row in private
    ), private

    summary: dict[str, object] = {
        "media": media,
        "assets": assets,
        "failure": failure,
        "models": model_results,
        "private": private,
    }
    (args.out / "qualified.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(
        json.dumps(
            {
                "qualified": str(args.out / "qualified.json"),
                "runs": [
                    item["run"].get("name", item["run"]["id"])
                    for item in [media, assets, failure, *model_results]
                ],
            }
        ),
        flush=True,
    )


if __name__ == "__main__":
    main()
