"""Actual stored bytes, CUDA work and authored invocation lifetimes; no mock backend."""

from __future__ import annotations

import gc
import hashlib
import importlib.metadata
import json
import shutil
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict
from pathlib import Path

import msgspec
import tensorfs
import torch
from cozy_runtime.author import App, Context, Device, Invocation
from cozy_runtime.author import prepare as prepare_request
from cozy_runtime.author._invoke import run_prepared
from cozy_runtime.compute import PreparedRepeat
from tensorfs import varena

MIB = 1 << 20
MODES = {
    "loaded",
    "cancel-before",
    "cancel-race",
    "cancel-active",
    "input-error",
    "scope-return",
    "scope-unwind",
    "index-stale",
}


class Empty(msgspec.Struct):
    pass


class Returned(msgspec.Struct):
    returned: bool


def until(execution, predicate, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        facts = execution.facts()
        if predicate(facts):
            return facts
        if facts["native_load"]["done"]:
            raise AssertionError(
                f"native work ended before the required boundary: {facts}"
            )
        time.sleep(0.001)
    raise TimeoutError(f"native boundary was not reached: {execution.facts()}")


def source(directory, nbytes):
    values = torch.full((nbytes // 4,), 0.125, dtype=torch.float32)
    writer = tensorfs.Writer(directory / "weights.cozytensors")
    writer.component("net").put("weight", values)
    checkpoint = writer.commit()
    digest = hashlib.sha256(values.numpy().tobytes()).hexdigest()
    return checkpoint, digest, [("net/weight#value", 0, nbytes)]


def private_input(checkpoint, directory):
    exported = Path(tensorfs.materialize(checkpoint, local_dir=directory / "export"))
    private = directory / "private-input"
    shutil.copytree(exported, private, copy_function=shutil.copyfile, symlinks=False)
    copied = tensorfs.open(str(private), local_files_only=True)
    chunks = [
        private / "chunks" / digest.removeprefix("sha256:")
        for digest, _ in copied.chunks
    ]
    assert chunks and all(p.is_file() and p.stat().st_nlink == 1 for p in chunks)
    return copied, chunks[-1]


def prepare(arena, checkpoint, parts, nbytes, steps, ordering="checked"):
    slot = varena.LoadSlot(arena, checkpoint, parts, bytes=nbytes, threads=1)
    alias = slot.tensor
    weight = alias.view(torch.float32)
    output = torch.zeros(1, device="cuda", dtype=torch.float32)
    calls = [0]
    indices = []

    def body(index):
        if not indices:
            indices.append(index)
        calls[0] += 1
        output.add_(weight.sum())

    plan = PreparedRepeat.capture(
        body, state=(output,), steps=steps, load=slot, load_ordering=ordering
    )
    assert plan.facts()["continuation_abi"] == 1
    assert calls[0] == 2, "the Python body must run only for warmup and capture"
    assert float(weight.sum()) == 0.0, "capture must precede the source load"
    assert float(output[0]) == 0.0
    assert arena.stats()["pinned_bytes"] >= nbytes
    assert arena.reclaim(nbytes) == 0, "preparation lost the dedicated slot's pin"
    return {
        "slot": slot,
        "alias": alias,
        "output": output,
        "calls": calls,
        "plan": plan,
        "index": indices[0],
    }


def close_checked(bundle, arena, nbytes):
    alias = bundle.pop("alias")
    bundle["plan"].close()
    bundle["slot"].close()
    bundle.clear()
    gc.collect()
    assert arena.reclaim(nbytes) == 0, "an escaped storage alias lost its physical pin"
    del alias
    gc.collect()
    arena.reclaim(nbytes)
    stats = arena.stats()
    assert stats["pinned_bytes"] == 0 and stats["resident_bytes"] == 0, stats
    assert not stats["poisoned"], stats
    return stats


def exercise(mode, seed, outer_context, tel, ordering="checked"):
    if mode not in MODES:
        raise ValueError(f"unknown native load gate mode: {mode}")
    varena.install_allocator()
    torch.cuda.set_device(0)
    torch.set_num_threads(1)
    if not hasattr(varena, "LoadSlot"):
        raise RuntimeError("installed TensorFS has no native LoadSlot capability")
    nbytes = (256 if mode == "input-error" else 64) * MIB
    steps = 30_000 if mode in {"cancel-race", "cancel-active", "scope-unwind"} else 30
    if mode == "scope-return":
        steps = 2_000
    arena = varena.Arena(0)
    evidence = {
        "mode": mode,
        "ordering": ordering,
        "seed": seed,
        "runtime": importlib.metadata.version("cozy-runtime"),
        "tensorfs": importlib.metadata.version("tensorfs"),
        "torch": torch.__version__,
        "device": torch.cuda.get_device_name(0),
        "source_bytes": nbytes,
        "requested_steps": steps,
    }
    with tempfile.TemporaryDirectory(prefix="cozy-native-load-execute-") as temporary:
        directory = Path(temporary)
        checkpoint, digest, parts = source(directory, nbytes)
        copied = damaged = None
        if mode == "input-error":
            copied, damaged = private_input(checkpoint, directory)
        bundle = prepare(arena, copied or checkpoint, parts, nbytes, steps, ordering)
        evidence["source_sha256"] = digest
        if mode == "cancel-before":
            bundle["slot"].cancel()
        app = App()

        @app.entrypoint
        def handler(payload: Empty, ctx: Context) -> Returned:
            execution = bundle["execution"] = ctx.plan(bundle["plan"])
            if mode == "scope-return":
                return Returned(True)  # Actual invocation closure must join this work.
            if mode in {"cancel-active", "scope-unwind"}:
                evidence["before_cancel"] = until(
                    execution,
                    lambda f: (
                        f["native_load"]["compute_wait_entered"]
                        and f["host_graph_launches"] == 1
                    ),
                )
                assert not execution.poll().done, "the active-cancel window was missed"
                assert arena.reclaim(nbytes) == 0, (
                    "running compute lost the loaded weight pin"
                )
                if mode == "scope-unwind":
                    raise ValueError("intentional handler unwind after native launch")
                waiting = threading.Event()

                def observe():
                    waiting.set()
                    return execution.wait()

                with ThreadPoolExecutor(max_workers=1) as pool:
                    observer = pool.submit(observe)
                    assert waiting.wait(1)
                    time.sleep(0.02)
                    began = time.monotonic()
                    execution.cancel()  # Must not wait behind the completion observer.
                    observer.result(timeout=10)
                evidence["cancel_and_join_ms"] = (time.monotonic() - began) * 1000
            elif mode == "cancel-race":
                execution.cancel()  # Native start/cancel must resolve this race safely.
            elif mode == "input-error":
                evidence["before_damage"] = until(
                    execution,
                    lambda f: (
                        f["native_load"]["issued_bytes"] > 0
                        and f["native_load"]["issued_bytes"] < nbytes
                        and not f["native_load"]["read_done"]
                    ),
                )
                assert damaged is not None
                with damaged.open("r+b") as handle:
                    handle.truncate(
                        0
                    )  # Only this fixture's independently copied source.
                try:
                    execution.wait()
                except tensorfs.Error as error:
                    evidence["source_error"] = {
                        "code": error.code,
                        "detail": str(error),
                    }
                    assert error.code in {"io", "integrity", "not_found"}, str(error)
                    raise
                raise AssertionError("the truncated owned source did not fail its read")
            else:
                # No poll, wait or Python callback can drive read → compute in this interval.
                parked = time.monotonic()
                time.sleep(3.0)
                evidence["parked_seconds"] = time.monotonic() - parked
                evidence["after_park"] = execution.facts()
                assert evidence["after_park"]["native_load"]["done"], (
                    "native work must finish while the submitting Python thread is parked"
                )
            assert arena.reclaim(nbytes) == 0, "active submission lost slot ownership"
            execution.wait()
            return Returned(True)

        try:
            if mode == "index-stale":
                assert bundle["plan"].valid()
                bundle["index"].fill_(73)
                assert bundle["plan"].valid(), (
                    "input values must not invalidate geometry"
                )
                bundle["index"].resize_(2)
                assert not bundle["plan"].valid()
                try:
                    bundle["plan"].submit()
                except RuntimeError as error:
                    assert "storage or geometry changed" in str(error), error
                    evidence["refusal"] = str(error)
                else:
                    raise AssertionError(
                        "native index resize was accepted before submission"
                    )
                evidence["native"] = bundle["plan"].facts()
                assert evidence["native"]["host_graph_launches"] == 0
                evidence["index_shape_refused_before_submit"] = True
                evidence["cleanup"] = close_checked(bundle, arena, nbytes)
                return evidence
            if mode in {"scope-return", "scope-unwind", "input-error"}:
                # The executor's actual prepare/invoke lifecycle, including checked teardown.
                # Expected errors are contained in this real invocation, not mocked away.
                result, outcome, _ = run_prepared(
                    prepare_request(app.get("handler"), {}),
                    Invocation(
                        request_id=f"native-load-{mode}-{seed}",
                        spool=directory / "attempt",
                        deadline=time.monotonic() + 90,
                        device=Device("cuda", 0),
                    ),
                )
                evidence["invocation"] = {
                    "terminal": outcome.terminal,
                    "origin": outcome.origin,
                    "code": outcome.code,
                }
                if mode == "scope-return":
                    assert result is not None and outcome.terminal == "succeeded", (
                        outcome
                    )
                elif mode == "scope-unwind":
                    assert (
                        outcome.terminal == "failed" and outcome.origin == "author"
                    ), outcome
                    assert "intentional handler unwind" in outcome.message, outcome
                else:
                    assert (
                        outcome.terminal == "failed" and "source_error" in evidence
                    ), outcome
            else:
                handler(Empty(), outer_context)

            execution = bundle["execution"]
            facts = execution.facts()
            evidence["native"] = facts
            load = facts["native_load"]
            assert facts["load_ordering_requested"] == ordering
            if ordering != "auto":
                assert load["ordering"] == facts["load_ordering"] == ordering
            assert load["done"] and load["completion_proven"], facts
            assert bundle["calls"][0] == 2, (
                "Python drove native continuation or loop steps"
            )
            evidence["python_body_calls"] = bundle["calls"][0]
            if mode == "input-error":
                assert load["failed"] and load["issued_bytes"] > 0, load
                assert facts["host_graph_launches"] == 0, facts
                assert float(bundle["output"][0]) == 0.0
                # Checked ordinary failure must leave the CUDA context usable.
                assert torch.equal(
                    torch.ones(16, device="cuda").add_(1).cpu(), torch.full((16,), 2.0)
                )
            else:
                progress = execution.poll()
                evidence["progress"] = asdict(progress)
                assert progress.done
                assert (
                    float(bundle["output"][0])
                    == (nbytes // 4) * 0.125 * progress.completed_steps
                )
                if mode.startswith("cancel-") or mode == "scope-unwind":
                    assert progress.canceled and progress.completed_steps < steps, (
                        progress
                    )
                    assert facts["host_graph_launches"] in (0, 1)
                else:
                    assert (
                        not progress.canceled and progress.completed_steps == steps
                    ), progress
                    assert facts["host_graph_launches"] == 1, facts
                    assert load["read_bytes"] == nbytes and load["issued_bytes"] > 0, (
                        load
                    )
                    if facts["load_ordering"] == "events":
                        assert (
                            0
                            < load["launch_started_ns"]
                            <= load["launch_submitted_ns"]
                            < load["read_completed_ns"]
                        ), load
                        assert (
                            facts["body_entry_checked"]
                            and facts["body_entered"]
                            and not facts["input_invalid"]
                        ), facts
                    else:
                        assert (
                            0
                            < load["read_completed_ns"]
                            <= load["launch_started_ns"]
                            <= load["compute_completed_ns"]
                        ), load
                    expected = (nbytes // 4) * 0.125 * steps
                    assert float(execution.wait()[0][0]) == expected
                    evidence["result"] = expected
                if mode == "cancel-before":
                    assert facts["host_graph_launches"] == load["issued_bytes"] == 0, (
                        facts
                    )
                if mode in {"cancel-active", "scope-unwind"}:
                    assert (
                        facts["host_graph_launches"] == 1
                        and load["compute_wait_entered"]
                    ), facts
            del execution
            evidence["cleanup"] = close_checked(bundle, arena, nbytes)
            with varena.capture(0):
                pass  # The native producer/capture exclusion must be released after completion.
            evidence["passed"] = True
            tel.log(json.dumps(evidence, sort_keys=True))
            return evidence
        finally:
            if bundle:
                bundle[
                    "plan"
                ].close()  # No abandonment of a live native owner on assertion failure.
                bundle["slot"].close()
            if copied is not None:
                copied.close()
            checkpoint.close()
