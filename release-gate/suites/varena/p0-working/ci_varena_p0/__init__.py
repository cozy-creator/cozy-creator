"""The ordinary Cozy call owns a fresh candidate child; the installed Runtime remains visible."""
from __future__ import annotations

import json
import re
from pathlib import Path
import subprocess
import sys

import msgspec
from cozy_runtime.author import App, Telemetry

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    torch_version: str = "2.14.0+cu130"
    torch_git: str = "08187d9e0fba026dc8217405802ab5381dc88d90"
    compute_sanitizer: str = ""
    seed: int = 138
    case: str = "all"


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=False)
def fork(payload: Probe, tel: Telemetry) -> Report:
    """A fresh CPU-only process installs callbacks, then proves inherited calls reject before CUDA."""
    if payload.compute_sanitizer:
        raise ValueError("fork proof initializes no CUDA and is not a sanitizer target")
    child = Path(__file__).with_name("fork_probe.py")
    result = None
    with tel.stage("varena-fork-before-cuda"):
        process = subprocess.Popen(
            [sys.executable, "-I", str(child), msgspec.json.encode(payload).decode()],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        assert process.stdout is not None
        try:
            for line in process.stdout:
                tel.log(line.rstrip())
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                if isinstance(record, dict) and "result" in record:
                    result = record["result"]
            code = process.wait()
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait()
    if code or not isinstance(result, dict) or not result.get("passed"):
        raise RuntimeError(f"fork-before-CUDA proof failed ({code}): {result}")
    return Report(True, json.dumps(result, sort_keys=True))


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, tel: Telemetry) -> Report:
    return _cuda_child(payload, tel, "probe.py", "varena-p0")


@app.entrypoint(accelerator=True)
def phase_policy(payload: Probe, tel: Telemetry) -> Report:
    """P2 keyed eager measurements and explicit raw admission; existing model policy is unchanged."""
    return _cuda_child(payload, tel, "phase_policy.py", "varena-p2-phase-policy")


@app.entrypoint(accelerator=True)
def replay_policy(payload: Probe, tel: Telemetry) -> Report:
    """P2 graph-definition traces validated by actual native continuation completion."""
    return _cuda_child(payload, tel, "replay_policy.py", "varena-p2-replay-policy")


@app.entrypoint(accelerator=True)
def raw_prefetch(payload: Probe, tel: Telemetry) -> Report:
    """Real repeated raw stages exercise measured identity-only source lookahead."""
    return _cuda_child(payload, tel, "raw_prefetch.py", "varena-p2-raw-prefetch")


@app.entrypoint(accelerator=True)
def native_cudnn(payload: Probe, tel: Telemetry) -> Report:
    """Fresh native-Torch cuDNN lifetime control, retaining the same strict sanitizer verdict."""
    return _cuda_child(payload, tel, "native_cudnn.py", "native-cudnn-lifetime")


def _cuda_child(payload: Probe, tel: Telemetry, filename: str, stage: str) -> Report:
    config = msgspec.json.encode(payload).decode()
    child = Path(__file__).with_name(filename)
    result = None
    command = [sys.executable, "-I", str(child), config]
    summaries = []
    if payload.compute_sanitizer:
        tool = Path(payload.compute_sanitizer)
        if not tool.is_absolute() or not tool.is_file():
            raise ValueError("compute_sanitizer must name an installed absolute executable")
        command = [str(tool), "--tool", "memcheck", "--leak-check", "full",
                   "--error-exitcode", "99", "--target-processes", "all", *command]
    with tel.stage(stage):
        process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        assert process.stdout is not None
        try:
            for line in process.stdout:
                tel.log(line.rstrip())
                tel.progress(0.0)
                if match := re.search(r"ERROR SUMMARY:\s+(\d+) errors", line):
                    summaries.append(int(match.group(1)))
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                if isinstance(record, dict) and "result" in record:
                    result = record["result"]
            code = process.wait()
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait()
    if payload.compute_sanitizer and isinstance(result, dict):
        # Error messages are size-limited. Keep the complete instrumented report even
        # when the tool rejects process teardown after all device cases have passed.
        result["passed"] = bool(not code and result.get("passed") and summaries and not any(summaries))
        result["compute_sanitizer"] = {
            "path": payload.compute_sanitizer, "exit_code": code, "error_summaries": summaries,
        }
        return Report(result["passed"], json.dumps(result, sort_keys=True))
    if code or not isinstance(result, dict) or not result.get("passed"):
        raise RuntimeError(f"{stage} child failed ({code}): {result}")
    return Report(True, json.dumps(result, sort_keys=True))


class Benchmark(msgspec.Struct, forbid_unknown_fields=True):
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    torch_version: str = "2.14.0+cu130"
    torch_git: str = "08187d9e0fba026dc8217405802ab5381dc88d90"
    samples: int = 7
    rounds: int = 50_000


@app.entrypoint(accelerator=True)
def benchmark(payload: Benchmark, tel: Telemetry) -> Report:
    if payload.samples < 5 or payload.rounds < 10_000:
        raise ValueError("allocator benchmark needs at least five samples and 10,000 rounds")
    child = Path(__file__).with_name("benchmark.py")
    results = {}
    for mode in ("native", "varena"):
        config = msgspec.to_builtins(payload)
        config["mode"] = mode
        result = None
        with tel.stage(f"allocator-{mode}"):
            process = subprocess.Popen(
                [sys.executable, "-I", str(child), json.dumps(config)],
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            )
            assert process.stdout is not None
            try:
                for line in process.stdout:
                    tel.log(line.rstrip())
                    tel.progress(0.0)
                    try:
                        record = json.loads(line)
                    except ValueError:
                        continue
                    if isinstance(record, dict) and "result" in record:
                        result = record["result"]
                code = process.wait()
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait()
        if code or not isinstance(result, dict) or not result.get("passed"):
            raise RuntimeError(f"allocator {mode} child failed ({code}): {result}")
        results[mode] = result
    native, candidate = results["native"], results["varena"]
    for key in ("tensorfs_version", "extension_sha256", "shim_sha256", "torch", "torch_git", "torch_distribution", "torch_cuda", "torch_cxx11_abi",
                "python", "executable", "gpu", "gpu_uuid", "capability", "visible_devices",
                "allocator_conf", "cuda_allocator_conf"):
        if native["provenance"][key] != candidate["provenance"][key]:
            raise RuntimeError(f"allocator benchmark provenance differs: {key}")
    for key in ("sizes", "stream", "sample_count", "pairs_per_sample", "warmup_pairs"):
        if native[key] != candidate[key]:
            raise RuntimeError(f"allocator benchmark settings differ: {key}")
    ratio = candidate["median_host_ns_per_pair"] / native["median_host_ns_per_pair"]
    completion_ratio = candidate["median_completion_ns_per_pair"] / native["median_completion_ns_per_pair"]
    report = {
        "passed": ratio <= 2.0, "host_pair_ratio": ratio,
        "completion_pair_ratio": completion_ratio, "host_pair_ratio_limit": 2.0,
        "comparison": "two fresh sequential children, native then Varena, same interpreter/device and identical raw alloc/free loop",
        "qualification": "allocator host-pair microbenchmark only; Python dispatch included; not model latency",
        "results": results,
    }
    tel.log(json.dumps({"allocator_benchmark": report}, sort_keys=True))
    # Preserve every failed sample in the result artifact; successful delivery is not a pass.
    return Report(report["passed"], json.dumps(report, sort_keys=True))
