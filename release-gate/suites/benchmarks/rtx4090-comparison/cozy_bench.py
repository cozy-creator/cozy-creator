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
            rgba = picture.convert("RGBA")
            alpha = rgba.getchannel("A")
            images.append(
                {
                    "path": str(path),
                    "bytes": path.stat().st_size,
                    "sha256": digest,
                    "format": picture.format,
                    "size": list(picture.size),
                    "rgb_sha256": hashlib.sha256(rgb.tobytes()).hexdigest(),
                    "rgba_sha256": hashlib.sha256(rgba.tobytes()).hexdigest(),
                    "mode": picture.mode, "alpha_extrema": alpha.getextrema(),
                    "alpha_nonopaque_pixels": sum(alpha.histogram()[:255]),
                    "rgb_extrema": rgb.getextrema(),
                }
            )
    assert len(images) == 1, images
    return images


def summarize(run, request, size, events, cli_wall_ms, step_stage):
    assert run["status"] == "completed", run.get("reason", run["status"])
    assert run["result"]["seed"] == request["seed"]
    assert (run["result"]["width"], run["result"]["height"]) == size
    measured = run["measurements"]
    assert measured["software"]["attempt_ordinal"] == 1, "retry is a separately reported sample"
    began_ms = run["started_at_ms"]
    assert run.get("execution_ms", 0) > 0
    execution = measured["execution"]
    gpu_rows = execution["gpus"]
    pids = sorted({gpu["pid"] for gpu in gpu_rows})
    assert len(pids) == 1 and type(pids[0]) is int and pids[0] > 0
    for gpu in gpu_rows:
        assert gpu["end_us"] > gpu["start_us"] >= (began_ms - 1000) * 1000
    # Run has no top-level memoized flag. Call records expose the flag; never turn
    # an absent field into false. Fresh execution evidence below is always required.
    own_calls = [call for call in run.get("calls", []) if call.get("run") == run["id"]]
    for call in own_calls:
        assert call.get("memoized") is False, ("own call was memoized or missing flag", call)

    def metric(name, required=False):
        found = [r for r in measured.get("observations", []) if r.get("name") == name]
        assert len(found) <= 1, (name, "duplicate observations")
        if not found:
            assert not required, (name, "missing")
            return None
        row = found[0]
        assert row["kind"] == "metric" and type(row["value"]) in (int, float)
        stamp = row.get("at_unix_ms")
        assert type(stamp) is int and stamp >= began_ms - 1000, (name, "stale metric")
        return row

    attribution = measured["attribution"]
    stages = attribution["stages"]
    steps = attribution.get("steps", {}).get(step_stage)
    native_preparation = attribution.get("steps", {}).get("native preparation step 0")
    observed_step_track = step_stage
    pipeline = metric("request_path.pipeline_native_steps")
    interval = metric("request_path.denoise_native_steps")
    pipeline_reused = None
    pipeline_launches = None
    if pipeline is not None:
        assert interval is None and pipeline["value"] == request["steps"]
        assert steps is None or steps["count"] == 0, "pipeline mixed with eager denoise evidence"
        stage = stages["native_pipeline"]
        assert stage["count"] == 1
        assert stage["ended_unix_ms"] >= stage["started_unix_ms"] >= began_ms - 1000
        launches = metric("request_path.pipeline_host_launches", True)
        reused = metric("request_path.pipeline_reused", True)
        assert launches["value"] == 1 and reused["value"] in (0, 1)
        pipeline_launches, pipeline_reused = 1, bool(reused["value"])
        mode = "resident_pipeline"
        callbacks, native_steps, native_rows = 0, int(pipeline["value"]), [pipeline]
        device = None
    else:
        if interval is not None and step_stage == "generating image":
            # Qwen attempts extract step 0 before capture. A refused capture reruns
            # the ordinary eager pipeline, so its attempt has a distinct real step
            # track rather than adding a misleading 31st denoise callback.
            assert steps is None or steps["count"] == 0, "native Qwen mixed with eager fallback callbacks"
            assert native_preparation is not None and native_preparation["count"] == 1
            steps = native_preparation
            observed_step_track = "native preparation step 0"
        assert steps is not None, "missing current denoise callback track"
        assert steps["ended_unix_ms"] >= steps["started_unix_ms"] >= began_ms - 1000
        callbacks = steps["count"]
        assert type(callbacks) is int and callbacks > 0
        native_steps = 0
        native_rows = []
        if interval is not None:
            value = interval["value"]
            assert 0 <= value <= request["steps"] and value == int(value)
            assert step_stage in ("denoise", "generating image") and callbacks == 1
            native_steps, native_rows = int(value), [interval]
        assert callbacks + native_steps == request["steps"]
        mode = "eager_step0_native_interval" if native_steps else "eager_steps"
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
    required_gaps = [gap for gap in gaps if not (native_steps and gap == "complete device-step timing series unavailable")]
    return {
        "run_id": run["id"], "run_number": run["number"], "seed": request["seed"],
        "mode": mode, "cli_wall_ms": cli_wall_ms,
        "request_created_to_settled_ms": run["settled_at_ms"] - run["created_at_ms"],
        "execution_ms_as_reported": run["execution_ms"],
        "cross_clock_accepted_to_started_delta_ms": run["started_at_ms"] - run["accepted_at_ms"],
        "cross_clock_note": "Caller accepted and worker started clocks differ; this delta is not queue latency.",
        "steps": callbacks + native_steps, "observed_step_callback_count": callbacks,
        "native_completed_steps": native_steps, "native_step_observations": native_rows,
        "pipeline_reused": pipeline_reused, "pipeline_host_launches": pipeline_launches,
        "step_timing_scope": "Resident pipeline stage includes encode, denoise and decode; native intervals have no complete per-step series. Never divide it into fabricated GPU-step times.",
        "step_stage": "native_pipeline" if pipeline is not None else step_stage,
        "observed_step_track": observed_step_track,
        "native_preparation_as_reported": native_preparation,
        "native_preparation_discarded": native_preparation is not None and interval is None,
        "device_steps_as_reported": device,
        "device_first_step_ms": samples[0] if samples else None,
        "device_steady_step_ms": {"count": len(steady), "mean": statistics.mean(steady),
                                  "median": statistics.median(steady), "min": min(steady), "max": max(steady)} if steady else None,
        "host_steps_as_reported": steps, "stages_as_reported": stages,
        "executor_initialization_as_reported": execution.get("executor"),
        "construction_as_reported": execution.get("construction"),
        "download_ms": None,
        "download_timing_note": "No download time inferred from unattributed or nested scopes.",
        "stage_events": [r for r in events if r.get("type") == "run.progress" or r.get("level") == "stage"],
        "executor_pids": pids, "gpu_intervals_as_reported": gpu_rows,
        "current_request_timing_verified": True,
        "own_call_memo_flags": [call["memoized"] for call in own_calls],
        "memo_evidence": "Observed own-call flags when present plus fresh current steps/native completion, current GPU interval and unique request seed; absent flags are not inferred false.",
        "models": measured["models"], "software": measured["software"],
        "routes": [r for r in measured.get("observations", []) if r.get("name", "").startswith(("gemm.", "attention."))],
        "blocking": [r for r in measured.get("observations", []) if r.get("name") == "request_path.blocking"],
        "plan_reports": execution.get("plan_reports"), "graphs_as_reported": graphs,
        "tracked_peak_allocated_bytes": memory.get("peak_allocated_bytes"),
        "tracked_peak_reserved_bytes": memory.get("peak_reserved_bytes"),
        "memory_as_reported": memory,
        "memory_scope_note": "Tracked process/native measurements, not a hard driver/machine VRAM cap.",
        "images": image_evidence(run, size), "metric_gaps": gaps,
        "required_metric_gaps": required_gaps, "device_step_timing_complete": len(samples) == request["steps"],
        "unmeasured": ["physical disk and PCIe traffic including canceled speculative reads",
                       "GPU idle time causally attributable to missing weights",
                       "isolated cold-weight latency excluding setup and compilation"],
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
    parser.add_argument("--require-native", action="store_true", help="Require native29 for resident qualification; omit on memory-constrained comparisons")
    parser.add_argument("--match-proof", required=True, type=Path)
    parser.add_argument("--target", required=True)
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--machine", required=True)
    parser.add_argument("--lane", required=True)
    parser.add_argument("--seeds", required=True, nargs=4, type=int)
    parser.add_argument("--steps", type=int, default=30, choices=range(20, 31))
    parser.add_argument("--step-stage", default="denoise")
    parser.add_argument("--expect-size", default="1024x1024")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument(
        "--resume",
        action="store_true",
        help="reuse saved completed receipts, submit only missing calls",
    )
    args = parser.parse_args()
    match = json.loads(args.match_proof.read_text())
    assert match["matched"] is True and match["steps"] == args.steps == 30
    assert len(set(args.seeds)) == 4, "distinct seeds prevent request memoization"
    size = tuple(int(value) for value in args.expect_size.split("x"))
    assert len(size) == 2 and all(0 < value <= 8192 for value in size)
    output = args.output.absolute()
    output.mkdir(parents=True, exist_ok=args.resume)
    cozy = str(Path.home() / ".cozy/bin/cozy")
    template = json.loads(args.input.read_text())
    paired = {row["seed"]: row for row in json.loads((Path(__file__).parent / "cards/resident-3090/requests.json").read_text())}
    assert set(args.seeds) == set(paired)
    source = source_identity(args.target)
    assert source["commit"] == "a3755916454acff2e941644776f74c1e371766ca"
    report = {
        "target": args.target,
        "source": source,
        "lane": args.lane,
        "machine": args.machine,
        "runs": [],
        "controller_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "controller_commit": subprocess.check_output(
            ["git", "-C", "/home/fidika/cozy/.worktrees/cozy-creator/varena-consolidation", "rev-parse", "HEAD"], text=True
        ).strip(),
    }
    previous = {}
    if args.resume:
        previous = json.loads((output / "report.json").read_text())
        for key in ("target", "source", "lane", "machine"):
            assert previous[key] == report[key], f"resume changed {key}"
        archive = output / f"report-before-resume-{time.time_ns()}.json"
        write(archive, previous)
        report["resumed_from"] = str(archive)
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
            assert source_identity(args.target) == source, (
                "package source changed during qualification"
            )
            request = {**template, "steps": args.steps, "seed": seed, "prompt": paired[seed]["prompt"], "negative_prompt": paired[seed]["negative_prompt"]}
            if args.resume and directory.exists():
                assert json.loads((directory / "request.json").read_text()) == request
                run = json.loads((directory / "run.ndjson").read_text())
                assert run["status"] == "completed", (
                    "resume never resubmits failed or unsettled work"
                )
                events = run_events(directory / "call.ndjson")
                timing = directory / "call-timing.json"
                elapsed = (
                    json.loads(timing.read_text())["cli_wall_ms"]
                    if timing.exists()
                    else next(
                        (
                            r.get("cli_wall_ms")
                            for r in previous["runs"]
                            if r.get("label") == label
                        ),
                        None,
                    )
                )
                status = 0
            else:
                directory.mkdir()
                write(directory / "request.json", request)
                status, elapsed = command(
                    cozy,
                    directory,
                    "call",
                    [
                        "run",
                        args.target,
                        "model=_/sdxl@sha256:df8494606a805c233a46b931174b9388208eb4ea0c83deb23fa833375d2956f5",
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
                write(
                    directory / "call-timing.json",
                    {"cli_wall_ms": elapsed, "exit_code": status},
                )
                events = run_events(directory / "call.ndjson")
                recorded = [
                    row["run"] for row in events if isinstance(row.get("run"), dict)
                ]
                assert recorded, f"no admitted run; CLI exit {status}, see {directory}"
                run = read_snapshot(
                    cozy,
                    directory,
                    "run",
                    ["run", "show", recorded[-1]["id"], "--once"],
                )
            if status or run["status"] != "completed":
                report["runs"].append(
                    {"label": label, "cli_wall_ms": elapsed, "run": run}
                )
                raise RuntimeError(
                    f"{label} did not complete; no further calls submitted"
                )
            row = summarize(run, request, size, events, elapsed, args.step_stage)
            assert run["result"]["prompt"] == request["prompt"]
            if args.require_native:
                assert row["observed_step_callback_count"] == 1 and row["native_completed_steps"] == 29
            assert row["software"]["runtime"] == "0.32.3+db7a54bb"
            assert row["software"]["tensord_commit"] == "8499b11b24a0c4222ab6daae58d9a75816591641"
            assert row["software"]["tensorfs"] == "0.15.8"
            assert any(m["manifest"] == "sha256:cbacec84cee812314251badfa5810bedcc31f143e4a874cc1323ae14bd480667" and m["lane"] == "bf16" for m in row["models"])
            row["label"] = label
            if elapsed is None:
                row["cli_wall_note"] = (
                    "Original controller failed before saving wall time; not reconstructed."
                )
            assert any(model["lane"] == args.lane for model in row["models"]), row[
                "models"
            ]
            report["runs"].append(row)
            if index:
                prior = report["runs"][-2]
                assert row["run_id"] != prior["run_id"]
                assert min(
                    g["start_us"] for g in row["gpu_intervals_as_reported"]
                ) >= max(g["end_us"] for g in prior["gpu_intervals_as_reported"]), (
                    "GPU interval reused or overlapping a preceding sequential request"
                )
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
        report["required_metric_gaps"] = {
            row["label"]: row["required_metric_gaps"]
            for row in report["runs"]
            if row["required_metric_gaps"]
        }
        report["measurements_complete"] = not report["metric_gaps"]
        report["passed"] = not report["required_metric_gaps"]
        report["machine_after"] = read_snapshot(
            cozy, output, "machine-after", ["machine", "show", args.machine]
        )
        assert report["passed"], report["required_metric_gaps"]
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
