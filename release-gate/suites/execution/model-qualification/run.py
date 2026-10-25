"""Sequential real image-model qualification through the default Cozy CLI.

First call plus distinct-seed warm calls; no installation, rental, private API or SDK mutation.
Run only after the machine's GPU lane has been handed to this qualification.
"""

import argparse
import hashlib
import json
import statistics
import subprocess
import time
from pathlib import Path


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def command(cozy, directory, name, arguments):
    with (
        (directory / f"{name}.ndjson").open("w") as out,
        (directory / f"{name}.stderr").open("w") as err,
    ):
        began = time.monotonic_ns()
        result = subprocess.run(
            [cozy, "--json", *arguments], stdout=out, stderr=err, check=False
        )
        elapsed_ms = (time.monotonic_ns() - began) / 1e6
    return result.returncode, elapsed_ms


def read_snapshot(cozy, directory, name, arguments):
    status, _ = command(cozy, directory, name, arguments)
    document = json.loads((directory / f"{name}.ndjson").read_text())
    if status and document.get("status") not in ("failed", "canceled"):
        raise RuntimeError(f"{name} failed; see its saved CLI output")
    return document


def run_events(path):
    rows = []
    for line in path.read_text().splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if isinstance(row, dict):
            rows.append(row)
    return rows


def image_evidence(run, size):
    from PIL import Image

    images = []
    for output in run["output"]:
        if not output.get("media_type", "").startswith("image/"):
            continue
        path = Path(output["path"])
        digest = "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()
        assert digest == output["sha256"], (path, digest, output["sha256"])
        with Image.open(path) as picture:
            picture.load()
            assert picture.size == size, (picture.size, size)
            rgb = picture.convert("RGB")
            images.append(
                {
                    "path": str(path),
                    "bytes": path.stat().st_size,
                    "sha256": digest,
                    "format": picture.format,
                    "size": list(picture.size),
                    "rgb_sha256": hashlib.sha256(rgb.tobytes()).hexdigest(),
                    "rgb_extrema": rgb.getextrema(),
                }
            )
    assert len(images) == 1, images
    return images


def summarize(run, request, size, events, cli_wall_ms):
    assert run["status"] == "completed", run.get("reason", run["status"])
    assert run["result"]["seed"] == request["seed"], run["result"]
    assert (run["result"]["width"], run["result"]["height"]) == size
    measured = run["measurements"]
    execution = measured["execution"]
    steps = measured["attribution"]["steps"]["denoise"]
    assert steps["count"] == request["steps"], steps
    gpu_rows = execution["gpus"]
    pids = sorted({gpu["pid"] for gpu in gpu_rows})
    assert len(pids) == 1 and isinstance(pids[0], int) and pids[0] > 0, pids
    device = steps.get("device")
    samples = device.get("series", []) if device else []
    steady = samples[1:]
    memory = measured["memory"]
    graphs = measured["residency"].get("graphs")
    gaps = []
    if len(samples) != request["steps"]:
        gaps.append("complete device-step timing series unavailable")
    for key in ("peak_allocated_bytes", "peak_reserved_bytes"):
        if key not in memory:
            gaps.append(f"{key} unavailable")
    if graphs is None:
        gaps.append("graph measurements unavailable")
    recorded_ms = run["settled_at_ms"] - run["created_at_ms"]
    return {
        "run_id": run["id"],
        "run_number": run["number"],
        "seed": request["seed"],
        "cli_wall_ms": cli_wall_ms,
        "request_created_to_settled_ms": recorded_ms,
        "execution_ms_as_reported": run.get("execution_ms"),
        "accepted_to_started_ms": run["started_at_ms"] - run["accepted_at_ms"],
        "steps": steps["count"],
        "device_steps_as_reported": device,
        "device_first_step_ms": samples[0] if samples else None,
        "device_steady_step_ms": {
            "count": len(steady),
            "mean": statistics.mean(steady),
            "median": statistics.median(steady),
            "min": min(steady),
            "max": max(steady),
        }
        if steady
        else None,
        "host_steps_as_reported": steps,
        "stages_as_reported": measured["attribution"].get("stages"),
        "executor_initialization_as_reported": execution.get("executor"),
        "construction_as_reported": execution.get("construction"),
        "download_ms": None,
        "download_timing_note": "No separate download duration is inferred from nested preparation scopes.",
        "stage_events": [
            r
            for r in events
            if r.get("type") == "run.progress" or r.get("level") == "stage"
        ],
        "executor_pids": pids,
        "gpu_intervals_as_reported": gpu_rows,
        "models": measured["models"],
        "software": measured["software"],
        "routes": [
            r
            for r in measured.get("observations", [])
            if r.get("name", "").startswith(("gemm.", "attention."))
        ],
        "plan_reports": execution.get("plan_reports"),
        "graphs_as_reported": graphs,
        "tracked_peak_allocated_bytes": memory.get("peak_allocated_bytes"),
        "tracked_peak_reserved_bytes": memory.get("peak_reserved_bytes"),
        "memory_as_reported": memory,
        "memory_scope_note": "Tracked process/native measurements; not a hard driver or machine VRAM cap.",
        "images": image_evidence(run, size),
        "metric_gaps": gaps,
    }


def source_identity(target):
    directory = Path(target).absolute().parent
    if not (directory / "pyproject.toml").is_file():
        return {"target": target, "kind": "published"}
    prefix = subprocess.check_output(
        ["git", "-C", str(directory), "rev-parse", "--show-prefix"], text=True
    ).strip("/\n")
    head = subprocess.check_output(
        ["git", "-C", str(directory), "rev-parse", "HEAD"], text=True
    ).strip()
    tree = subprocess.check_output(
        ["git", "-C", str(directory), "rev-parse", f"HEAD:{prefix}"], text=True
    ).strip()
    dirty = subprocess.check_output(
        ["git", "-C", str(directory), "status", "--porcelain", "--", "."], text=True
    )
    assert not dirty, "qualify committed package contents only"
    return {"target": target, "kind": "source", "commit": head, "package_tree": tree}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--machine", required=True)
    parser.add_argument("--lane", required=True)
    parser.add_argument("--seeds", required=True, nargs=4, type=int)
    parser.add_argument("--steps", type=int, default=30, choices=range(20, 31))
    parser.add_argument("--expect-size", default="1024x1024")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    assert len(set(args.seeds)) == 4, "distinct seeds prevent request memoization"
    size = tuple(int(value) for value in args.expect_size.split("x"))
    assert len(size) == 2 and all(0 < value <= 8192 for value in size)
    output = args.output.absolute()
    output.mkdir(parents=True, exist_ok=False)
    cozy = str(Path.home() / ".cozy/bin/cozy")
    template = json.loads(args.input.read_text())
    source = source_identity(args.target)
    report = {
        "target": args.target,
        "source": source,
        "lane": args.lane,
        "machine": args.machine,
        "runs": [],
    }
    try:
        before = read_snapshot(
            cozy, output, "machine-before", ["machine", "show", args.machine]
        )
        assert before["state"] == "ready" and not before["status"].get("runs"), (
            "machine GPU lane is not idle"
        )
        queued = read_snapshot(
            cozy,
            output,
            "queued-before",
            ["run", "list", "--on", args.machine, "--state", "queued", "--limit", "0"],
        )
        assert not queued["runs"], "machine already has queued work"
        report["machine_before"] = before
        report["cold_definition"] = (
            "first observed call; disk/source coldness is reported separately and not forced"
        )
        for index, seed in enumerate(args.seeds):
            label = "cold" if index == 0 else f"warm-{index}"
            directory = output / label
            directory.mkdir()
            assert source_identity(args.target) == source, (
                "package source changed during qualification"
            )
            request = {**template, "steps": args.steps, "seed": seed}
            write(directory / "request.json", request)
            status, elapsed = command(
                cozy,
                directory,
                "call",
                [
                    "run",
                    args.target,
                    "--input",
                    str(directory / "request.json"),
                    "--lane",
                    args.lane,
                    "--on",
                    args.machine,
                    "--gpus=1",
                    "--out",
                    str(directory / "assets"),
                ],
            )
            events = run_events(directory / "call.ndjson")
            recorded = [
                row["run"] for row in events if isinstance(row.get("run"), dict)
            ]
            assert recorded, f"no admitted run; CLI exit {status}, see {directory}"
            run = read_snapshot(
                cozy, directory, "run", ["run", "show", recorded[-1]["id"], "--once"]
            )
            if status or run["status"] != "completed":
                report["runs"].append(
                    {"label": label, "cli_wall_ms": elapsed, "run": run}
                )
                raise RuntimeError(
                    f"{label} did not complete; no further calls submitted"
                )
            row = summarize(run, request, size, events, elapsed)
            row["label"] = label
            assert any(model["lane"] == args.lane for model in row["models"]), row[
                "models"
            ]
            report["runs"].append(row)
            if index:
                assert all(
                    row["software"][key] == report["runs"][0]["software"][key]
                    for key in ("runtime", "tensorfs", "tensord_commit")
                ), "software cohort changed"
                row["executor_reused"] = (
                    row["executor_pids"] == report["runs"][0]["executor_pids"]
                )
                assert row["executor_reused"], "warm executor reuse not established"
            write(output / "report.json", report)
        report["metric_gaps"] = {
            row["label"]: row["metric_gaps"]
            for row in report["runs"]
            if row["metric_gaps"]
        }
        report["passed"] = not report["metric_gaps"]
        report["machine_after"] = read_snapshot(
            cozy, output, "machine-after", ["machine", "show", args.machine]
        )
        assert report["passed"], report["metric_gaps"]
    except BaseException as error:
        report["passed"] = False
        report["error"] = repr(error)
        raise
    finally:
        write(output / "report.json", report)
    print(
        json.dumps({"passed": True, "report": str(output / "report.json")}), flush=True
    )


if __name__ == "__main__":
    main()
