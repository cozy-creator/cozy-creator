"""Owning raw-read readiness, cancellation and ordinary-input-error qualification.

No device faults are injected. Error inputs are independent copies owned by this cell.
"""

from __future__ import annotations

import hashlib
import importlib.metadata
import json
import shutil
import sys
import tempfile
import time
from pathlib import Path

import tensorfs
import torch
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.internal import accel, residency
from tensorfs import varena
from torch import nn

MIB = 1 << 20


def source(directory: Path, nbytes: int):
    values = torch.arange(nbytes // 4, dtype=torch.int32).remainder_(1009)
    writer = tensorfs.Writer(directory / "model.cozytensors")
    writer.component("next").put("data", values)
    checkpoint = writer.commit()
    return checkpoint, values, [("next/data#value", 0, nbytes)]


def copied_values(region, expected):
    stream = torch.cuda.Stream()
    with torch.cuda.stream(stream):
        raw = region.prepare().tensor
        actual = raw.view(torch.int32).cpu()
    torch.testing.assert_close(actual, expected, rtol=0, atol=0)
    del raw
    return hashlib.sha256(actual.numpy().tobytes()).hexdigest()


def wait_issued(read):
    while True:
        row = read.poll()
        if row["issued_bytes"]:
            assert not row["done"], (
                "read completed before the requested in-flight action"
            )
            return row
        assert not row["done"], "read completed without issued-copy evidence"
        time.sleep(0.001)


def overlap(memory, directory, seed):
    checkpoint, expected, parts = source(directory / "future", 32 * MIB)
    generator = torch.Generator().manual_seed(seed)
    width = 1024
    matrix = torch.randn(width, width, generator=generator) / width**0.5
    writer = tensorfs.Writer(directory / "active.cozytensors")
    writer.component("active").put("weight", matrix)
    active = writer.commit()
    with torch.device("meta"):
        module = nn.Linear(width, width, bias=False)
    vars(module)["_cozy_weights"] = residency.Declared(
        residency.Weights(active, "active"), blocks=[]
    )
    owner = memory.load(module, "ram")
    binding = owner.retain()
    inputs = torch.randn(512, width, generator=generator).to("cuda")
    state = torch.empty_like(inputs)
    count = 3000

    def step(index):
        state.copy_(torch.relu(module(inputs)))

    plan = None
    try:
        with binding.prepare(), torch.no_grad():
            oracle = torch.relu(module(inputs)).clone()
            plan = PreparedRepeat.capture(
                step,
                state=(state,),
                steps=count,
                owners=(binding, active, module, inputs),
            )
        region = memory.native.reserve(expected.nbytes)
        read = region.prepare_read(checkpoint, parts, threads=2)
        # Destination plus the actual bounded staging/streams are prepared before launch.
        running = plan.submit()
        before = running.poll()
        while before.completed_steps == 0 and not before.done:
            time.sleep(0.001)
            before = running.poll()
        assert not before.done
        began = time.perf_counter_ns()
        read.start()
        completed = read.wait()
        read_ms = (time.perf_counter_ns() - began) / 1e6
        after = running.poll()
        assert completed["done"] and not completed["canceled"]
        assert before.completed_steps < after.completed_steps < count
        assert not after.done, (
            "raw publication joined the unrelated resident compute loop"
        )
        (output,) = running.wait()
        torch.testing.assert_close(output, oracle, rtol=0, atol=0)
        digest = copied_values(region, expected)
        assert memory.native.stats()["pinned_bytes"] == sum(
            b.region.allocated_bytes for b in owner.groups() if b.region is not None
        )
        reclaimed = memory.native.reclaim(region.allocated_bytes)
        assert reclaimed >= region.allocated_bytes and not region.resident
        padded = memory.native.reserve(expected.nbytes + 512)
        padded_read = padded.prepare_read(
            checkpoint, [("next/data#value", 256, expected.nbytes)], threads=2
        )
        padded_read.start()
        padded_read.wait()
        raw = padded.prepare().tensor
        copied = raw.cpu()
        assert torch.count_nonzero(copied[:256]).item() == 0
        assert torch.count_nonzero(copied[-256:]).item() == 0
        torch.testing.assert_close(
            copied[256:-256].view(torch.int32), expected, rtol=0, atol=0
        )
        del raw
        memory.native.reclaim(padded.allocated_bytes)
        return {
            "producer": completed,
            "read_ms": read_ms,
            "compute_steps_before": before.completed_steps,
            "compute_steps_at_publication": after.completed_steps,
            "compute_total_steps": count,
            "cross_stream_sha256": digest,
            "reclaimed_future_bytes": reclaimed,
            "verified_padding_bytes": 512,
            "scope": "raw producer publication while a resident matmul loop remains incomplete; no decode-overlap claim",
        }
    finally:
        if plan is not None:
            plan.close()
        binding.close()
        owner.close()
        memory.trim()
        active.close()
        checkpoint.close()


def cancellation(memory, directory):
    checkpoint, expected, parts = source(directory, 256 * MIB)
    region = memory.native.reserve(expected.nbytes)
    before_start = region.prepare_read(checkpoint, parts, threads=1)
    before_start.cancel()
    canceled = before_start.wait()
    assert canceled["done"] and canceled["canceled"] and not canceled["started"]
    assert canceled["issued_bytes"] == 0 and not region.resident
    assert memory.native.stats()["pinned_bytes"] == 0
    first_reclaim = memory.native.reclaim(region.allocated_bytes)
    assert first_reclaim >= region.allocated_bytes
    # Keep the canceled token alive: it must not retain an idle Fill pin.
    during = region.prepare_read(checkpoint, parts, threads=1)
    during.start()
    issued = wait_issued(during)
    began = time.perf_counter_ns()
    during.cancel()
    ended = during.wait()
    cancel_ms = (time.perf_counter_ns() - began) / 1e6
    assert ended["done"] and ended["canceled"]
    assert 0 < ended["issued_bytes"] < expected.nbytes
    assert not region.resident and memory.native.stats()["pinned_bytes"] == 0
    reclaimed = memory.native.reclaim(region.allocated_bytes)
    checkpoint.close()
    return {
        "before_start": canceled,
        "before_start_reclaimed_bytes": first_reclaim,
        "issued_before_cancel": issued,
        "after_cancel": ended,
        "cancel_to_checked_completion_ms": cancel_ms,
        "reclaimed_bytes": reclaimed,
        "canceled_tokens_retained_without_pins": True,
    }


def input_error(memory, directory):
    checkpoint, expected, parts = source(directory / "original", 256 * MIB)
    exported = Path(tensorfs.materialize(checkpoint, local_dir=directory / "export"))
    private = directory / "private-error-input"
    shutil.copytree(exported, private, copy_function=shutil.copyfile, symlinks=False)
    bad = tensorfs.open(str(private), local_files_only=True)
    chunk_paths = [
        private / "chunks" / digest.removeprefix("sha256:") for digest, _ in bad.chunks
    ]
    assert chunk_paths and all(
        path.is_file() and path.stat().st_nlink == 1 for path in chunk_paths
    )
    region = memory.native.reserve(expected.nbytes)
    read = region.prepare_read(bad, parts, threads=1)
    read.start()
    issued = wait_issued(read)
    # The source is an owned, independent copy. Never mutate a shared store object/link.
    damaged = chunk_paths[-1]
    with damaged.open("r+b") as handle:
        handle.truncate(0)
    try:
        read.wait()
    except tensorfs.Error as error:
        failure = {
            "type": type(error).__name__,
            "code": error.code,
            "detail": str(error),
        }
        assert error.code in {"io", "integrity", "not_found"}
    else:
        raise AssertionError("the owned truncated input did not fail its actual read")
    assert not region.resident and memory.native.stats()["pinned_bytes"] == 0
    assert not memory.native.stats()["poisoned"]
    with varena.capture(0):
        pass  # checked failure must release the owning producer lease
    # The failed recipe never became ready; a healthy copy of the same source can fill anew.
    retry = region.prepare_read(checkpoint, parts, threads=2)
    retry.start()
    success = retry.wait()
    digest = copied_values(region, expected)
    reclaimed = memory.native.reclaim(region.allocated_bytes)
    assert memory.native.stats()["pinned_bytes"] == 0
    bad.close()
    checkpoint.close()
    return {
        "issued_before_input_error": issued,
        "failure": failure,
        "retry": success,
        "retry_sha256": digest,
        "reclaimed_bytes": reclaimed,
        "error_input_is_private_copy": True,
    }


def exercise(seed, mode):
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    assert varena.supports_refill(), "candidate has no native owning refill interface"
    memory = residency.Memory.open("cuda:0")
    with tempfile.TemporaryDirectory(prefix="cozy-owned-refill-") as directory:
        root = Path(directory)
        result = {
            "async": lambda: overlap(memory, root, seed),
            "async-cancel": lambda: cancellation(memory, root),
            "async-error": lambda: input_error(memory, root),
        }[mode]()
    return {
        "passed": True,
        "mode": mode,
        "seed": seed,
        "runtime_version": importlib.metadata.version("cozy-runtime"),
        "tensorfs_version": tensorfs.__version__,
        "result": result,
    }


if __name__ == "__main__":
    print(json.dumps({"result": exercise(int(sys.argv[1]), sys.argv[2])}), flush=True)
