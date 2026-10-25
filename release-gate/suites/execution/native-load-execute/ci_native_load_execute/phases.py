"""Real stored tensor phases and native cancellation through the installed GPU SDK."""

import gc
import importlib.metadata
import re
import tempfile
import time
from pathlib import Path

import tensorfs
import torch
from cozy_runtime.author import CapabilityError
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.internal import accel
from tensorfs import varena

from .exercise import private_input

MODES = {
    "phase-repeat",
    "phase-prefix",
    "phase-suffix",
    "phase-both",
    "phase-reuse",
    "phase-loaded",
    "phase-input-error",
    "phase-cancel-prefix",
    "phase-cancel-final",
    "phase-cancel-tail",
    "phase-refuse-prefix",
    "phase-refuse-suffix",
}


def exercise(mode, seed, ctx):
    assert ctx.device.type == "cuda"
    accel.initialize(torch, "cuda")
    loaded = mode in {"phase-loaded", "phase-input-error"}
    arena = varena.Arena(torch.cuda.current_device()) if loaded else None
    result = _exercise(mode, seed, ctx, arena)
    if arena is not None:
        gc.collect()
        arena.reclaim(8448)
        stats = arena.stats()
        assert stats["pinned_bytes"] == stats["resident_bytes"] == 0, stats
        assert not stats["poisoned"], stats
        result["after_scope_release"] = stats
    return result


def _exercise(mode, seed, ctx, arena):
    if mode not in MODES:
        raise ValueError(mode)
    assert ctx.device.type == "cuda"
    accel.initialize(torch, "cuda")
    device = torch.cuda.current_device()
    loaded = mode in {"phase-loaded", "phase-input-error"}
    has_prefix = mode not in {"phase-repeat", "phase-suffix", "phase-cancel-final"}
    has_suffix = mode not in {"phase-repeat", "phase-prefix"}
    steps = 1 if mode == "phase-cancel-final" else 30
    calls = {"prefix": 0, "body": 0, "suffix": 0}
    evidence = {"mode": mode, "seed": seed, "steps": steps}

    with tempfile.TemporaryDirectory(prefix="cozy-real-phases-") as directory:
        directory = Path(directory)
        with torch.inference_mode(False), torch.no_grad():
            # Three actual stored components, packed into one optional checked LoadSlot.
            cpu = [torch.eye(32), torch.full((32,), 0.5), torch.eye(32) * 0.25]
            writer = tensorfs.Writer(directory / "phases.cozytensors")
            for name, value in zip(("prefix", "repeat", "suffix"), cpu, strict=True):
                writer.component(name).put("weight", value)
            checkpoint = writer.commit()
            source = checkpoint
            damaged = None
            if mode == "phase-input-error":
                source, damaged = private_input(checkpoint, directory)
            slot = None
            if loaded:
                slot = varena.LoadSlot(
                    arena,
                    source,
                    [
                        ("prefix/weight#value", 0, 4096),
                        ("repeat/weight#value", 4096, 128),
                        ("suffix/weight#value", 4352, 4096),
                    ],
                    bytes=8448,
                    threads=2,
                )
                raw = slot.tensor
                weights = [
                    raw[:4096].view(torch.float32).view(32, 32),
                    raw[4096:4224].view(torch.float32),
                    raw[4352:].view(torch.float32).view(32, 32),
                ]
            else:
                weights = [
                    torch.from_dlpack(
                        source.components[name].tensors.read(
                            "weight", device=f"cuda:{device}"
                        )
                    )
                    for name in ("prefix", "repeat", "suffix")
                ]
                raw = None
            inputs = (
                torch.arange(64, dtype=torch.float32).view(2, 32) / 64 + seed % 13
            ).to(device)
            conditioned = (
                inputs.clone() if not has_prefix else torch.full_like(inputs, -23)
            )
            latent = torch.zeros_like(inputs)
            result = torch.full_like(inputs, -17)
            counters = torch.zeros(3, dtype=torch.int32, device=device)
            state = [conditioned, latent, result, counters]
            heavy_a = heavy_b = heavy_weight = None
            if mode in {
                "phase-cancel-prefix",
                "phase-cancel-final",
                "phase-cancel-tail",
            }:
                # Actual GEMMs make a cooperative-cancellation window. No sleep kernel,
                # mock driver, injected status or host callback enters the graph.
                writer = tensorfs.Writer(directory / "long-phase.cozytensors")
                writer.component("work").put("weight", torch.eye(512))
                long_source = writer.commit()
                heavy_weight = torch.from_dlpack(
                    long_source.components["work"].tensors.read(
                        "weight", device=f"cuda:{device}"
                    )
                )
                heavy_a = torch.eye(512, device=device)
                heavy_b = torch.zeros_like(heavy_a)
                state.extend((heavy_a, heavy_b))
            host = None
            if mode in {"phase-refuse-prefix", "phase-refuse-suffix"}:
                host = torch.full(inputs.shape, 7.0, pin_memory=True)
            original = [value.clone() for value in state]

            def long_phase():
                for _ in range(512):
                    torch.mm(heavy_a, heavy_weight, out=heavy_b)
                    heavy_a.copy_(heavy_b)

            def prefix():
                calls["prefix"] += 1
                torch.mm(inputs, weights[0], out=conditioned)
                conditioned.add_(1)
                counters[0].add_(1)
                if mode == "phase-cancel-prefix":
                    long_phase()
                if mode == "phase-refuse-prefix":
                    conditioned.copy_(host, non_blocking=True)

            def body(index):
                del index
                calls["body"] += 1
                latent.add_(conditioned * weights[1])
                counters[1].add_(1)
                if mode == "phase-cancel-final":
                    long_phase()

            def suffix():
                calls["suffix"] += 1
                torch.mm(latent, weights[2], out=result)
                counters[2].add_(1)
                if mode == "phase-cancel-tail":
                    long_phase()
                if mode == "phase-refuse-suffix":
                    result.copy_(host, non_blocking=True)

            plan = None
            try:
                try:
                    plan = PreparedRepeat.capture(
                        body,
                        state=state,
                        steps=steps,
                        load=slot,
                        prefix=prefix if has_prefix else None,
                        suffix=suffix if has_suffix else None,
                    )
                except CapabilityError as error:
                    if mode not in {"phase-refuse-prefix", "phase-refuse-suffix"}:
                        raise
                    assert error.code == "prepared_graph_ineligible"
                    cause = error.__cause__
                    assert cause is not None
                    assert getattr(cause, "eligibility_refusal", False) is True
                    assert not hasattr(cause, "error_code"), str(cause)
                    reason = str(cause)
                    metadata = dict(re.findall(r"\b([a-z_]+)=([^\s]+)", reason))
                    assert metadata.get("stage") == mode.removeprefix(
                        "phase-refuse-"
                    ), reason
                    explicit_h2d = (
                        metadata.get("reason") == "memcpy_layout_or_kind"
                        and metadata.get("copy_kind") == "1"
                    )
                    default_host = (
                        metadata.get("reason") == "memcpy_source"
                        and metadata.get("copy_kind") == "4"
                        and metadata.get("pointer_type") == "1"
                    )
                    assert explicit_h2d or default_host, reason
                    assert int(metadata["expected_device"]) == device
                    for actual, saved in zip(state, original, strict=True):
                        torch.testing.assert_close(actual, saved, rtol=0, atol=0)
                    assert calls == {"prefix": 2, "body": 2, "suffix": 2}, calls
                    evidence.update(
                        refusal=reason, inspection=metadata, rollback=True, launches=0
                    )
                else:
                    assert mode not in {"phase-refuse-prefix", "phase-refuse-suffix"}, (
                        "host-copy phase was admitted"
                    )
                    expected_calls = {
                        "prefix": 2 if has_prefix else 0,
                        "body": 2,
                        "suffix": 2 if has_suffix else 0,
                    }
                    assert calls == expected_calls
                    for actual, saved in zip(state, original, strict=True):
                        torch.testing.assert_close(actual, saved, rtol=0, atol=0)
                    if damaged is not None:
                        with damaged.open("r+b") as target:
                            target.truncate(
                                0
                            )  # Only this gate's private, nonlinked store copy.
                    count = 2 if mode == "phase-reuse" else 1
                    runs = []
                    for iteration in range(count):
                        if iteration:
                            inputs.add_(2)
                            conditioned.fill_(-23)
                            latent.zero_()
                            result.fill_(-17)
                            counters.zero_()
                        # One ctx.plan per invocation; the first reuse uses public
                        # explicit submission, joined below and closed in finally.
                        execution = (
                            plan.submit()
                            if iteration < count - 1 or mode == "phase-input-error"
                            else ctx.plan(plan)
                        )
                        if mode in {"phase-cancel-prefix", "phase-cancel-final"}:
                            execution.cancel()
                        if mode == "phase-cancel-tail":
                            deadline = time.monotonic() + 10
                            while True:
                                observed = execution.poll()
                                if observed.done:
                                    raise AssertionError(
                                        "post-loop cancellation window "
                                        "was not exercised"
                                    )
                                if observed.completed_steps == steps:
                                    break
                                if time.monotonic() >= deadline:
                                    raise TimeoutError(
                                        "native loop did not reach its suffix window"
                                    )
                                time.sleep(0.0001)
                            evidence["before_tail_cancel"] = {
                                "completed": observed.completed_steps,
                                "done": observed.done,
                            }
                            execution.cancel()
                        if mode == "phase-input-error":
                            try:
                                execution.wait()
                            except tensorfs.Error as error:
                                facts = execution.facts()
                                assert facts["host_graph_launches"] == 0, facts
                                assert facts["native_load"]["completion_proven"], facts
                                for actual, saved in zip(state, original, strict=True):
                                    torch.testing.assert_close(
                                        actual, saved, rtol=0, atol=0
                                    )
                                evidence.update(
                                    error=type(error).__name__,
                                    failed_load=facts,
                                    all_phases_unchanged=True,
                                )
                            else:
                                raise AssertionError(
                                    "truncated real source unexpectedly succeeded"
                                )
                            break
                        if mode not in {
                            "phase-cancel-prefix",
                            "phase-cancel-final",
                            "phase-cancel-tail",
                        }:
                            time.sleep(
                                0.1
                            )  # Watcher parked; it does not drive any phase.
                        outputs = execution.wait()
                        progress = execution.poll()
                        assert progress.done
                        if mode in {"phase-cancel-prefix", "phase-cancel-final"}:
                            assert progress.canceled, (mode, progress)
                            completed = 0 if mode == "phase-cancel-prefix" else 1
                            assert progress.completed_steps == completed, progress
                            assert outputs[3].tolist() == [
                                int(has_prefix),
                                completed,
                                0,
                            ]
                            torch.testing.assert_close(
                                outputs[2], torch.full_like(result, -17), rtol=0, atol=0
                            )
                        else:
                            assert progress.completed_steps == 30
                            skipped_tail = (
                                mode == "phase-cancel-tail" and progress.canceled
                            )
                            if mode == "phase-cancel-tail":
                                evidence["tail_cancel_boundary"] = (
                                    "skipped_before_suffix"
                                    if skipped_tail
                                    else "suffix_already_admitted"
                                )
                            else:
                                assert not progress.canceled
                            prepared = (
                                inputs.cpu() @ cpu[0] + 1
                                if has_prefix
                                else inputs.cpu()
                            )
                            expected = prepared * cpu[1] * steps
                            torch.testing.assert_close(
                                outputs[1].cpu(), expected, rtol=0, atol=0
                            )
                            if has_suffix and not skipped_tail:
                                torch.testing.assert_close(
                                    outputs[2].cpu(), expected @ cpu[2], rtol=0, atol=0
                                )
                            if skipped_tail:
                                torch.testing.assert_close(
                                    outputs[2],
                                    torch.full_like(result, -17),
                                    rtol=0,
                                    atol=0,
                                )
                            assert outputs[3].tolist() == [
                                int(has_prefix),
                                steps,
                                int(has_suffix and not skipped_tail),
                            ]
                        assert calls == expected_calls, (
                            "native execution called a Python phase"
                        )
                        facts = execution.facts()
                        assert facts["host_graph_launches"] == iteration + 1
                        assert (
                            facts["has_prefix"] is has_prefix
                            and facts["has_suffix"] is has_suffix
                        )
                        runs.append(
                            {
                                "completed": progress.completed_steps,
                                "canceled": progress.canceled,
                                "counters": outputs[3].tolist(),
                                "native": facts,
                            }
                        )
                    evidence.update(runs=runs, capture_calls=calls, reusable=count == 2)
            finally:
                if plan is not None:
                    plan.close()
                if slot is not None:
                    slot.close()
            # Healthy work after checked close, never after an unproven fault.
            torch.testing.assert_close(inputs + 1, inputs.clone() + 1, rtol=0, atol=0)
            torch.cuda.synchronize()
            if arena is not None:
                # The slot alias stays live until all views/closure owners leave.
                held = arena.stats()
                if mode == "phase-loaded":
                    assert held["pinned_bytes"] > 0, held
                    assert held["resident_bytes"] >= held["pinned_bytes"], held
                    assert arena.reclaim(8448) == 0, (
                        "escaped raw views lost their physical pin"
                    )
                # Failed unpublished fill storage is never read or assumed resident.
                evidence["pins_before_scope_release"] = held

    evidence.update(
        runtime=importlib.metadata.version("cozy-runtime"),
        tensorfs=tensorfs.__version__,
        torch=str(torch.__version__),
        device=torch.cuda.get_device_name(device),
        scope=(
            "Resident composition correctness; "
            "no low-VRAM streaming or model benchmark claim"
        ),
    )
    return evidence
