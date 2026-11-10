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
from collections.abc import Mapping
from pathlib import Path
from typing import Literal, NotRequired, Required, TypeAlias, TypedDict, cast

JSON: TypeAlias = str | int | float | bool | None | list['JSON'] | dict[str, 'JSON']
Scalar: TypeAlias = str | int | float | bool | None


# The run record's fields read here (`cozy --json run show <id> --once`).
class Output(TypedDict, total=False):
    media_type: str
    path: Required[str]
    sha256: Required[str]


class Result(TypedDict):
    seed: int
    width: int
    height: int


class GpuInterval(TypedDict):
    pid: int
    start_us: int
    end_us: int


class Execution(TypedDict, total=False):
    gpus: Required[list[GpuInterval]]
    executor: JSON
    construction: JSON
    plan_reports: JSON


class Device(TypedDict, total=False):
    series: list[float]


class Steps(TypedDict, total=False):
    count: Required[int]
    started_unix_ms: Required[int]
    ended_unix_ms: Required[int]
    device: Device | None


class Attribution(TypedDict, total=False):
    steps: Required[dict[str, Steps]]
    stages: JSON


class Observation(TypedDict, total=False):
    name: str
    kind: str
    value: Scalar
    at_unix_ms: int


class Residency(TypedDict, total=False):
    graphs: JSON


class Model(TypedDict, total=False):
    lane: str


class Measurements(TypedDict, total=False):
    execution: Required[Execution]
    attribution: Required[Attribution]
    observations: list[Observation]
    memory: Required[dict[str, JSON]]
    residency: Required[Residency]
    models: Required[list[Model]]
    software: Required[dict[str, str]]


class RunRecord(TypedDict, total=False):
    id: Required[str]
    number: Required[int]
    status: Required[str]
    reason: JSON
    result: Required[Result]
    measurements: Required[Measurements]
    created_at_ms: Required[int]
    accepted_at_ms: Required[int]
    started_at_ms: Required[int]
    settled_at_ms: Required[int]
    execution_ms: int
    output: Required[list[Output]]


class RunRef(TypedDict):
    id: str


class Event(TypedDict, total=False):
    """A line of a call's ndjson stream."""
    type: str
    level: str
    run: RunRef


class MachineStatus(TypedDict, total=False):
    runs: JSON


class Machine(TypedDict):
    """`machine show`, the fields read here."""
    state: str
    status: MachineStatus


class Queued(TypedDict):
    """`run list`, the field read here."""
    runs: list[JSON]


class Request(TypedDict, total=False):
    """The input template, with this call's steps and seed."""
    steps: Required[int]
    seed: Required[int]


class ImageEvidence(TypedDict):
    path: str
    bytes: int
    sha256: str
    format: str | None
    size: list[int]
    rgb_sha256: str
    rgb_extrema: tuple[float, float] | tuple[tuple[float, float], ...]


class Steady(TypedDict):
    count: int
    mean: float
    median: float
    min: float
    max: float


class Summary(TypedDict):
    """One completed call in the report."""
    run_id: str
    run_number: int
    seed: JSON
    cli_wall_ms: float | None
    request_created_to_settled_ms: int
    execution_ms_as_reported: int | None
    accepted_to_started_ms: int
    steps: int
    observed_step_callback_count: int
    native_completed_steps: int
    native_step_observations: list[Observation]
    step_timing_scope: str
    step_stage: str
    device_steps_as_reported: Device | None
    device_first_step_ms: float | None
    device_steady_step_ms: Steady | None
    host_steps_as_reported: Steps
    stages_as_reported: JSON
    executor_initialization_as_reported: JSON
    construction_as_reported: JSON
    download_ms: None
    download_timing_note: str
    stage_events: list[Event]
    executor_pids: list[int]
    gpu_intervals_as_reported: list[GpuInterval]
    current_request_timing_verified: bool
    models: list[Model]
    software: dict[str, str]
    routes: list[Observation]
    plan_reports: JSON
    graphs_as_reported: JSON
    tracked_peak_allocated_bytes: JSON
    tracked_peak_reserved_bytes: JSON
    memory_as_reported: dict[str, JSON]
    memory_scope_note: str
    images: list[ImageEvidence]
    metric_gaps: list[str]
    required_metric_gaps: list[str]
    device_step_timing_complete: bool
    unmeasured: list[str]
    label: NotRequired[str]
    cli_wall_note: NotRequired[str]
    executor_reused: NotRequired[bool]


class Failed(TypedDict):
    """A call that did not complete: the report's last row before the controller raises."""
    label: str
    cli_wall_ms: float | None
    run: RunRecord


class Source(TypedDict, total=False):
    target: Required[str]
    kind: Required[str]
    commit: str
    package_tree: str


class Report(TypedDict, total=False):
    target: str
    source: Source
    lane: str
    machine: str
    runs: list[Summary | Failed]
    controller_sha256: str
    controller_commit: str
    resumed_from: str
    machine_before: Machine
    cold_definition: str
    metric_gaps: dict[str, list[str]]
    required_metric_gaps: dict[str, list[str]]
    measurements_complete: bool
    passed: bool
    machine_after: Machine
    error: str


class Args(argparse.Namespace):
    target: str
    input: Path
    machine: str
    lane: str
    seeds: list[int]
    steps: int
    step_stage: str
    expect_size: str
    output: Path
    resume: bool


def write(path: Path, value: Mapping[str, object]) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n")


def command(cozy: str, directory: Path, name: str, arguments: list[str]) -> tuple[int, float]:
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


def read_snapshot(cozy: str, directory: Path, name: str, arguments: list[str]) -> dict[str, JSON]:
    status, _ = command(cozy, directory, name, arguments)
    document: dict[str, JSON] = json.loads((directory / f"{name}.ndjson").read_text())
    if status and document.get("status") not in ("failed", "canceled"):
        raise RuntimeError(f"{name} failed; see its saved CLI output")
    return document


def run_events(path: Path) -> list[Event]:
    rows: list[Event] = []
    for line in path.read_text().splitlines():
        try:
            row: Event = json.loads(line)
        except ValueError:
            continue
        if isinstance(row, dict):
            rows.append(row)
    return rows


def image_evidence(run: RunRecord, size: tuple[int, ...]) -> list[ImageEvidence]:
    from PIL import Image

    images: list[ImageEvidence] = []
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


def summarize(run: RunRecord, request: Request, size: tuple[int, ...], events: list[Event], cli_wall_ms: float | None,
              step_stage: str) -> Summary:
    assert run["status"] == "completed", run.get("reason", run["status"])
    assert run["result"]["seed"] == request["seed"], run["result"]
    assert (run["result"]["width"], run["result"]["height"]) == size
    measured = run["measurements"]
    execution = measured["execution"]
    steps = measured["attribution"]["steps"][step_stage]
    native_rows = [
        row
        for row in measured.get("observations", [])
        if row.get("name") == "request_path.denoise_native_steps"
    ]
    assert len(native_rows) <= 1, "duplicate native step observations"
    native_steps = 0
    if native_rows:
        row = native_rows[0]
        value = cast(float, row.get("value"))  # a number: asserted next
        assert row.get("kind") == "metric" and type(value) in (int, float), row
        assert 0 <= value <= request["steps"] and value == int(value), row
        native_steps = int(value)
        assert step_stage == "denoise" and steps["count"] == 1, (
            "native interval requires exactly one observed eager denoise step",
            steps,
            row,
        )
    assert steps["count"] + native_steps == request["steps"], (
        "requested steps lack complete callback or retained native-step evidence",
        steps,
        native_rows,
    )
    gpu_rows = execution["gpus"]
    pids = sorted({gpu["pid"] for gpu in gpu_rows})
    assert len(pids) == 1 and isinstance(pids[0], int) and pids[0] > 0, pids
    # A memoized result must not qualify as a new inference merely by carrying old stats.
    # The worker supplies its invocation/step/GPU clocks. Compare within that clock
    # domain: real rental clocks can differ from the controller by tens of seconds.
    began_ms = run["started_at_ms"]
    assert run.get("execution_ms", 0) > 0, "no measured execution"
    for row in native_rows:
        stamp = row.get("at_unix_ms")
        assert type(stamp) is int and stamp >= began_ms - 1000, (
            "stale or unstamped native step observation",
            row,
        )
    for gpu in gpu_rows:
        assert gpu["end_us"] >= gpu["start_us"]
        assert gpu["start_us"] >= (began_ms - 1000) * 1000, "stale GPU interval"
    assert steps["started_unix_ms"] >= began_ms - 1000, "stale denoising measurements"
    assert steps["ended_unix_ms"] >= steps["started_unix_ms"]
    device = steps.get("device")
    samples = device.get("series", []) if device else []
    steady = samples[1:]
    memory = measured["memory"]
    graphs = measured["residency"].get("graphs")
    gaps: list[str] = []
    if len(samples) != request["steps"]:
        gaps.append("complete device-step timing series unavailable")
    for key in ("peak_allocated_bytes", "peak_reserved_bytes"):
        if key not in memory:
            gaps.append(f"{key} unavailable")
    if graphs is None:
        gaps.append("graph measurements unavailable")
    required_gaps = [
        gap
        for gap in gaps
        if not (
            native_steps and gap == "complete device-step timing series unavailable"
        )
    ]
    recorded_ms = run["settled_at_ms"] - run["created_at_ms"]
    return {
        "run_id": run["id"],
        "run_number": run["number"],
        "seed": request["seed"],
        "cli_wall_ms": cli_wall_ms,
        "request_created_to_settled_ms": recorded_ms,
        "execution_ms_as_reported": run.get("execution_ms"),
        "accepted_to_started_ms": run["started_at_ms"] - run["accepted_at_ms"],
        "steps": steps["count"] + native_steps,
        "observed_step_callback_count": steps["count"],
        "native_completed_steps": native_steps,
        "native_step_observations": native_rows,
        "step_timing_scope": "Only Python callback intervals are timed individually; native interval timing is included in the stage total.",
        "step_stage": step_stage,
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
        "current_request_timing_verified": True,
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
        "required_metric_gaps": required_gaps,
        "device_step_timing_complete": len(samples) == request["steps"],
        "unmeasured": [
            "physical disk and PCIe traffic, including canceled speculative reads",
            "GPU idle time attributable to missing weights",
            "isolated cold-weight latency excluding executor setup and compilation",
        ],
    }


def source_identity(target: str) -> Source:
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


def main() -> None:
    parser = argparse.ArgumentParser()
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
    args = parser.parse_args(namespace=Args())
    assert len(set(args.seeds)) == 4, "distinct seeds prevent request memoization"
    size = tuple(int(value) for value in args.expect_size.split("x"))
    assert len(size) == 2 and all(0 < value <= 8192 for value in size)
    output = args.output.absolute()
    output.mkdir(parents=True, exist_ok=args.resume)
    cozy = str(Path.home() / ".cozy/bin/cozy")
    template = json.loads(args.input.read_text())
    source = source_identity(args.target)
    report: Report = {
        "target": args.target,
        "source": source,
        "lane": args.lane,
        "machine": args.machine,
        "runs": [],
        "controller_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "controller_commit": subprocess.check_output(
            ["git", "-C", str(Path(__file__).parent), "rev-parse", "HEAD"], text=True
        ).strip(),
    }
    previous: Report = {}
    if args.resume:
        previous = json.loads((output / "report.json").read_text())
        key: Literal["target", "source", "lane", "machine"]
        for key in ("target", "source", "lane", "machine"):
            assert previous[key] == report[key], f"resume changed {key}"
        archive = output / f"report-before-resume-{time.time_ns()}.json"
        write(archive, previous)
        report["resumed_from"] = str(archive)
    # A run belongs to a project.
    project = output / "project"
    if not (project / "project.yaml").is_file():
        subprocess.run([cozy, "project", "new", str(project)], check=True, stdout=subprocess.DEVNULL)
    try:
        before = cast(Machine, read_snapshot(  # machine show
            cozy, output, "machine-before", ["machine", "show", args.machine]
        ))
        assert before["state"] == "ready" and not before["status"].get("runs"), (
            "machine GPU lane is not idle"
        )
        queued = cast(Queued, read_snapshot(  # run list
            cozy,
            output,
            "queued-before",
            ["run", "list", "--on", args.machine, "--state", "queued", "--limit", "0"],
        ))
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
            request: Request = {**template, "steps": args.steps, "seed": seed}
            if args.resume and directory.exists():
                assert json.loads((directory / "request.json").read_text()) == request
                run: RunRecord = json.loads((directory / "run.ndjson").read_text())
                assert run["status"] == "completed", (
                    "resume never resubmits failed or unsettled work"
                )
                events = run_events(directory / "call.ndjson")
                timing = directory / "call-timing.json"
                elapsed: float | None = (
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
                        "--input",
                        str(directory / "request.json"),
                        "--lane",
                        args.lane,
                        "--on",
                        args.machine,
                        "--gpus=1",
                        "-p",
                        str(project),
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
                run = cast(RunRecord, read_snapshot(  # run show
                    cozy,
                    directory,
                    "run",
                    ["run", "show", recorded[-1]["id"], "--once"],
                ))
            if status or run["status"] != "completed":
                report["runs"].append(
                    {"label": label, "cli_wall_ms": elapsed, "run": run}
                )
                raise RuntimeError(
                    f"{label} did not complete; no further calls submitted"
                )
            row = summarize(run, request, size, events, elapsed, args.step_stage)
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
                prior = cast(Summary, report["runs"][-2])  # a failed call raised above
                assert row["run_id"] != prior["run_id"]
                assert min(
                    g["start_us"] for g in row["gpu_intervals_as_reported"]
                ) >= max(g["end_us"] for g in prior["gpu_intervals_as_reported"]), (
                    "GPU interval reused or overlapping a preceding sequential request"
                )
                assert all(
                    row["software"][key] == cast(Summary, report["runs"][0])["software"][key]
                    for key in ("runtime", "tensorfs", "tensord_commit")
                ), "software cohort changed"
                row["executor_reused"] = (
                    row["executor_pids"] == cast(Summary, report["runs"][0])["executor_pids"]
                )
                assert row["executor_reused"], "warm executor reuse not established"
            write(output / "report.json", report)
        # Every call completed: a failed one raised.
        report["metric_gaps"] = {
            row["label"]: row["metric_gaps"]
            for row in cast(list[Summary], report["runs"])
            if row["metric_gaps"]
        }
        report["required_metric_gaps"] = {
            row["label"]: row["required_metric_gaps"]
            for row in cast(list[Summary], report["runs"])
            if row["required_metric_gaps"]
        }
        report["measurements_complete"] = not report["metric_gaps"]
        report["passed"] = not report["required_metric_gaps"]
        report["machine_after"] = cast(Machine, read_snapshot(  # machine show
            cozy, output, "machine-after", ["machine", "show", args.machine]
        ))
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
