# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow>=12", "av>=19"]
# ///
"""Actual CPU CLI acceptance for returned images and the existing output custody path."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from fractions import Fraction
from pathlib import Path
from typing import TypedDict, cast

import av
from PIL import Image


class Reason(TypedDict, total=False):
    code: str
    message: str


class OutputItem(TypedDict, total=False):
    url: str
    export_error: str
    media_type: str
    sha256: str
    length: int
    width: int
    height: int


class ChildCall(TypedDict, total=False):
    function: str
    status: str
    run: str
    reason: Reason


class Run(TypedDict, total=False):
    kind: str
    id: str
    name: str
    number: int
    status: str
    calls: list[ChildCall]
    reason: Reason
    output: list[OutputItem]
    result: dict[str, object]


class Event(TypedDict, total=False):
    type: str
    run: Run
    call: ChildCall
    item: OutputItem


@dataclass(frozen=True)
class Case:
    name: str
    function: str
    inputs: tuple[str, ...] = ()
    error: str | None = None
    message: str = ""
    image_format: str = "WEBP"
    size: tuple[int, int] = (4, 3)
    pixel: tuple[int, ...] = (15, 63, 127, 95)
    metadata: bool = True
    child: str = ""
    child_function: str = "generate"
    forwarded: bool = False


CASES = (
    Case("default", "generate"),
    Case("png", "generate", ("format=png",), image_format="PNG"),
    Case("webp", "generate", ("format=webp",)),
    Case(
        "large-side",
        "generate",
        ("width=16384", "height=1"),
        image_format="PNG",
        size=(16384, 1),
    ),
    Case(
        "encode-failure",
        "generate",
        ("width=16384", "height=1", "format=webp"),
        error="result_custody_failed",
        message="Image size exceeds WebP limit",
    ),
    Case("nested", "nested", size=(2, 2), pixel=(7, 11, 19), metadata=False),
    Case("child-forward", "child_forward", child="succeeded", forwarded=True),
    Case("child-nested", "child_nested", child="succeeded", forwarded=True),
    Case(
        "child-failure",
        "child_failure",
        error="result_custody_failed",
        message="Image size exceeds WebP limit",
        child="failed",
    ),
    Case("intermediate", "intermediate"),
    Case("streaming", "streaming", image_format="", size=(16, 16), metadata=False),
    Case("duplicate", "duplicate", error="duplicate_handle"),
    Case("decoded-bound", "decoded_bound", error="result_decoded_bound"),
    Case(
        "encoded-bound",
        "encoded_bound",
        error="result_custody_failed",
        message="over its declared 1-byte bound",
    ),
    Case(
        "encoded-bound-copy",
        "encoded_bound_copy",
        error="result_custody_failed",
        message="over its declared 1-byte bound",
    ),
    Case("invalid-shape", "invalid_shape", error="frame_shape"),
    Case("invalid-kind", "invalid_kind", error="frame_type"),
    Case("foreign", "foreign", error="foreign_handle"),
    Case("empty-format", "empty_format", error="encoder_unavailable"),
    Case("aliased-return", "aliased_return", error="duplicate_handle"),
    Case("aliased-child", "aliased_child", error="duplicate_handle", child="succeeded"),
    Case("api-removed", "api_removed"),
    Case(
        "child-input", "child_input", child="succeeded", child_function="inspect_image"
    ),
    Case(
        "child-input-label",
        "child_input_label",
        child="succeeded",
        child_function="inspect_image",
    ),
    Case("pending-probe", "pending_probe"),
    Case("publish-copy", "publish_copy"),
    Case("duplicate-label", "duplicate_label", error="duplicate_handle"),
)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def file_digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def assets(value: object) -> list[dict[str, object]]:
    if isinstance(value, dict):
        record = cast(dict[str, object], value)  # JSON object keys are strings.
        if "asset_ref" in record:
            return [record]
        return [asset for child in record.values() for asset in assets(child)]
    if isinstance(value, list):
        return [asset for child in value for asset in assets(child)]
    return []


def verify(
    case: Case, code: int, events: list[Event], exported: Path
) -> dict[str, object]:
    terminals = [
        event["run"]
        for event in events
        if event.get("type") in ("run.completed", "run.failed", "run.canceled")
        and event.get("run", {}).get("kind") == "call"
    ]
    require(
        bool(terminals),
        "no terminal call receipt: setup/installation errors are not expected failures",
    )
    run = terminals[-1]
    require(bool(run.get("id")), "terminal call has no run identity")
    files = sorted(path for path in exported.rglob("*") if path.is_file())
    calls = [
        *run.get("calls", []),
        *(event["call"] for event in events if event.get("type") == "run.call"),
    ]
    if case.child:
        child = [
            call
            for call in calls
            if call.get("function", "").endswith(case.child_function)
            and call.get("status") == case.child
            and call.get("run")
        ]
        require(
            bool(child),
            f"no identified {case.child_function} child with status {case.child}",
        )
        if case.child == "failed":
            require(
                any(
                    (call.get("reason") or {}).get("code") == case.error
                    for call in child
                ),
                "child failed for another reason",
            )
    summary: dict[str, object] = {
        "run_id": run["id"],
        "run_name": run.get("name"),
        "number": run.get("number"),
        "status": run.get("status"),
    }
    if case.error:
        require(
            code != 0 and run.get("status") == "failed",
            "expected a failed call and nonzero CLI exit",
        )
        reason = run.get("reason") or {}
        require(
            reason.get("code") == case.error, f"expected {case.error}, got {reason}"
        )
        require(
            case.message in reason.get("message", ""),
            "failure lost its expected codec/bound diagnostic",
        )
        require(
            not run.get("output") and not assets(run.get("result")),
            "failed call exposed a successful media result",
        )
        require(not files, "failed call exported final bytes")
        return {**summary, "reason": reason}
    require(
        code == 0 and run.get("status") == "completed",
        "expected successful call and CLI exit",
    )
    output = run.get("output", [])
    require(len(output) == 1, f"expected one final output, got {len(output)}")
    item = output[0]
    require(
        bool(item.get("url")) and not item.get("export_error"),
        "final output did not land in custody",
    )
    media_type = (
        "video/mp4"
        if case.name == "streaming"
        else f"image/{case.image_format.lower()}"
    )
    require(
        item.get("media_type") == media_type,
        f"wrong stored media type: {item.get('media_type')}",
    )
    require(len(files) == 1, f"expected one exported final file, got {len(files)}")
    path = files[0]
    digest, length = file_digest(path), path.stat().st_size
    require(
        length > 0 and item.get("length") == length,
        "export length differs from custody receipt",
    )
    require(
        str(item.get("sha256", "")).removeprefix("sha256:") == digest,
        "export digest differs from custody receipt",
    )
    leaves = assets(run.get("result"))
    require(len(leaves) == 1, "result does not contain exactly one file reference")
    leaf = leaves[0]
    require(
        leaf.get("digest") == f"sha256:{digest}",
        "result file digest differs from exported bytes",
    )
    # Root results retain their opaque output handle. Deferred frames may omit
    # size_bytes; the landed output item is authoritative for the encoded length.
    require(
        "size_bytes" not in leaf or leaf["size_bytes"] == length,
        "reported result length differs from exported bytes",
    )
    require(
        isinstance(leaf.get("asset_ref"), str)
        and bool(leaf["asset_ref"])
        and leaf.get("media_type") == media_type,
        "result file reference/type differs from custody",
    )
    if case.forwarded:
        require(
            leaf["asset_ref"] == f"sha256:{digest}"
            and leaf.get("size_bytes") == length,
            "forwarded child grant lost its canonical reference or encoded length",
        )
    if item.get("width") is not None or item.get("height") is not None:
        require(
            (item.get("width"), item.get("height")) == case.size,
            "stored output geometry differs from pixels",
        )
    if case.name == "streaming":
        with av.open(str(path)) as container:
            require(
                len(container.streams.video) == 1, "stream has no unique video track"
            )
            stream = container.streams.video[0]
            require(stream.codec_context.name == "h264", "stream did not encode H.264")
            frames = list(container.decode(stream))
            require(
                len(frames) == 3
                and all((frame.width, frame.height) == case.size for frame in frames),
                "stream frame count/geometry changed",
            )
            timestamps = []
            for frame in frames:
                require(
                    frame.pts is not None and frame.time_base is not None,
                    "stream frame has no clock",
                )
                assert frame.pts is not None and frame.time_base is not None
                timestamps.append(frame.pts * frame.time_base)
            require(
                timestamps == [Fraction(i, 24) for i in range(3)],
                "stream frame clock changed",
            )
        require(
            run.get("result", {}).get("frames") == 3,
            "result lost streaming frame facts",
        )
    else:
        with Image.open(path) as image:
            image.load()
            mode = "RGBA" if len(case.pixel) == 4 else "RGB"
            require(
                image.format == case.image_format,
                f"decoded container is {image.format}",
            )
            require(
                image.size == case.size and image.mode == mode,
                "export geometry or alpha mode changed",
            )
            require(
                image.tobytes() == bytes(case.pixel) * (case.size[0] * case.size[1]),
                "exported pixels differ from the immutable snapshot",
            )
        if case.name not in ("nested", "child-nested"):
            result = run.get("result") or {}
            require(
                (result.get("width"), result.get("height")) == case.size,
                "result geometry differs from exported pixels",
            )
        result = run.get("result") or {}
        if case.name == "api-removed":
            require(
                result.get("save_image_absent") is True,
                "removed save_image API remains callable",
            )
        if case.name in (
            "child-input",
            "child-input-label",
            "pending-probe",
            "publish-copy",
        ):
            require(
                result.get("probed") == list(case.size),
                "pending probe lost captured geometry",
            )
        if case.name in ("child-input", "child-input-label", "publish-copy"):
            require(
                result.get("original_digest") == f"sha256:{digest}",
                "original value cannot read adopted copy bytes",
            )
        if case.name in ("child-input", "child-input-label"):
            observed = result.get("seen")
            require(isinstance(observed, dict), "child returned no image facts")
            seen = cast(dict[str, object], observed)
            require(
                seen.get("digest") == f"sha256:{digest}",
                "child saw different encoded bytes",
            )
            require(
                (seen.get("width"), seen.get("height")) == case.size
                and seen.get("pixel") == list(case.pixel),
                "child decoded different image pixels",
            )
        if case.metadata:
            require(
                leaf.get("metadata")
                == {"case": "returned", "width": case.size[0], "height": case.size[1]},
                "file metadata was lost or changed",
            )
        if case.name == "intermediate":
            published = [
                event.get("item", {}).get("media_type")
                for event in events
                if event.get("type", "").startswith("output_item.")
            ]
            require(
                "image/png" in published
                and "image/webp" in published
                and published.index("image/png")
                < len(published) - 1 - published[::-1].index("image/webp"),
                "intermediate PNG and final WebP were not both published",
            )
    return {
        **summary,
        "sha256": digest,
        "length": length,
        "media_type": media_type,
        "export": str(path),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cozy", default="cozy")
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument(
        "--fixture",
        type=Path,
        required=True,
        help="copied, locked fixture pinned to an immutable SDK",
    )
    parser.add_argument(
        "--out",
        type=Path,
        required=True,
        help="new receipt directory; existing evidence is never overwritten",
    )
    parser.add_argument(
        "--sdk-identity",
        required=True,
        help="candidate commit or wheel SHA256 recorded by its build owner",
    )
    args = parser.parse_args()
    require("COZY_HOME" not in os.environ, "use the default Cozy home; unset COZY_HOME")
    cli = shutil.which(args.cozy)
    if cli is None:
        raise AssertionError(f"CLI not found: {args.cozy}")
    project: Path = args.project.resolve()
    fixture: Path = args.fixture.resolve()
    out: Path = args.out.resolve()
    require(project.is_dir(), "choose an existing normal Cozy project")
    require((fixture / "uv.lock").is_file(), "copy and lock the fixture before running")
    require(
        not out.is_relative_to(fixture), "keep receipts outside the fixture source tree"
    )
    out.mkdir(parents=True, exist_ok=False)
    inventory = {
        str(path.relative_to(fixture)): file_digest(path)
        for path in [
            fixture / "pyproject.toml",
            fixture / "uv.lock",
            *sorted((fixture / "images").glob("*.py")),
        ]
    }
    (out / "provenance.json").write_text(
        json.dumps(
            {
                "sdk_identity": args.sdk_identity,
                "cli": cli,
                "cli_sha256": file_digest(Path(cli)),
                "project": str(project),
                "fixture": str(fixture),
                "fixture_files": inventory,
            },
            indent=2,
        )
        + "\n"
    )
    summary: list[dict[str, object]] = []
    for case in CASES:
        exported = out / "exports" / case.name
        command = [
            cli,
            "run",
            str(fixture / case.function),
            *case.inputs,
            "--on",
            "local",
            "--json",
            "--full",
            "-o",
            str(exported),
        ]
        print(json.dumps({"case": case.name, "command": command}), flush=True)
        record: dict[str, object] = {
            "case": case.name,
            "command": command,
            "expected_error": case.error,
        }
        with (
            (out / f"{case.name}.ndjson").open("w") as stdout,
            (out / f"{case.name}.stderr").open("w") as stderr,
        ):
            completed = subprocess.run(
                command, cwd=project, stdout=stdout, stderr=stderr
            )
        record["exit_code"] = completed.returncode
        try:
            events = [
                cast(Event, json.loads(line))
                for line in (out / f"{case.name}.ndjson").read_text().splitlines()
                if line.strip()
            ]
            record.update(verify(case, completed.returncode, events, exported))
            record["passed"] = True
        except Exception as error:
            record.update(
                passed=False, verification_error=f"{type(error).__name__}: {error}"
            )
        summary.append(record)
        (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
        print(json.dumps(record), flush=True)
    return 0 if all(record["passed"] for record in summary) else 1


if __name__ == "__main__":
    sys.exit(main())
