"""Actual checkpoints, loader placement, residency, native graphs and CUDA allocations."""

from __future__ import annotations

import importlib.metadata
import tempfile
from pathlib import Path
from types import SimpleNamespace

import tensorfs
import torch
from cozy_runtime.author import CapabilityError
from cozy_runtime.author._loader import census
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.internal import accel, kernel_ready, residency
from cozy_runtime.internal.encoding import launch_providers
from cozy_runtime.internal.encoding.selection import measure_device, measure_runtime
from cozy_runtime.internal.fill import Checkpoint, dtype_name
from cozy_runtime.internal.lora_composition import composer
from cozy_runtime.internal.lora_contract import FORMAT, Graph, Linear
from cozy_runtime.internal.resolution import ConstructionFacts, Variant, resolve
from cozy_runtime.internal.weights import Backend, Placement
from torch import nn


def exercise(seed, ctx):
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open("cuda:0")
    width = 128
    plans = []
    with tempfile.TemporaryDirectory(prefix="cozy-resident-owner-") as directory:
        writer = tensorfs.Writer(Path(directory) / "weights.cozytensors")
        matrix = torch.eye(width) * 0.5
        for name in ("net", "next"):
            writer.component(name).put("weight", matrix)
        checkpoint = writer.commit()
        opened = Checkpoint(checkpoint)
        rows = [row for name in ("net", "next") for row in opened.rows(name)]
        facts = measure_device(torch, 0, "cuda")
        resolution = resolve(
            variant=Variant(
                "resident-owner",
                directory,
                checkpoint.id,
                {name: checkpoint.id for name in ("net", "next")},
            ),
            rows=rows,
            providers=launch_providers(),
            device=facts,
            runtime=measure_runtime(torch, importlib.metadata.version("cozy-runtime")),
            construction=ConstructionFacts(
                "resident-owner-proof", "Pair", ("net", "next")
            ),
            encoded_leaves="refuse",
            dtype_name=dtype_name,
        )
        with torch.device("meta"):
            built = SimpleNamespace(
                net=nn.Linear(width, width, bias=False),
                next=nn.Linear(width, width, bias=False),
            )
        walked = census(built)
        backend = Backend(
            {name: opened for name in ("net", "next")},
            rows,
            torch=torch,
            plan=resolution,
            memory=memory,
            device=torch.device("cuda:0"),
            construction="Pair",
            device_facts=facts,
            release="resident-owner-proof",
        )
        backend.expect(
            {
                name: [row.key for row in rows if row.component == name]
                for name in ("net", "next")
            }
        )
        fit = backend.fit(walked, encoded_leaves="refuse")
        assert fit and fit["ok"], fit
        live = backend.materialize(built, walked, encoded_leaves="refuse")
        for destination in walked.destinations:
            backend.fill(destination.key, destination.spec, live[destination.key])
        backend.commit()
        placement = Placement(memory, backend)
        with torch.inference_mode(False):
            state = torch.ones((2, width), device="cuda")
        calls = [0]

        def step(index):
            del index
            calls[0] += 1
            state.copy_(built.net(state))

        def capture():
            plan = PreparedRepeat.capture(
                step, state=(state,), steps=4, owners=(built.net,), resident_only=True
            )
            plans.append(plan)
            return plan

        def refusal(action):
            try:
                action()
            except CapabilityError as error:
                assert error.code == "prepared_graph_ineligible", str(error)
                return str(error)
            raise AssertionError("expected a prelaunch capability refusal")

        try:
            # The ordinary CLI executor already owns a real request. Opening a second
            # non-idle request here would itself mark both as overlapping before capture.
            request = residency._REQUEST.get()
            assert request is not None and request.memory is memory and not request.idle
            request.plan([backend.models["net"], backend.models["next"]])
            before_overlap = request.overlap
            assert not before_overlap, "the fixture needs one admitted GPU request"
            cold_reason = refusal(capture)
            assert calls[0] == 0 and request.moved == 0, request.stats()
            with torch.no_grad():
                torch.testing.assert_close(
                    built.net(state), state * 0.5, rtol=0, atol=0
                )
            before = request.moved
            plan = capture()
            assert calls[0] == 2 and request.moved == before
            assert request.overlap == before_overlap, (
                "idle capture changed request overlap"
            )
            output = ctx.plan(plan).wait()[0]
            torch.testing.assert_close(
                output, torch.full_like(state, 0.0625), rtol=0, atol=0
            )
            assert calls[0] == 2 and plan.facts()["host_graph_launches"] == 1
            plan.close()
            with torch.no_grad():
                torch.testing.assert_close(
                    built.next(state), state * 0.5, rtol=0, atol=0
                )
            assert len(request.phase_changes) == 2, request.phase_changes
            assert request.overlap == before_overlap
            phase_changes = list(request.phase_changes)

            # Supported ownership rejects Python hooks and variable per-step LoRA before warmup.
            handle = built.net.register_forward_pre_hook(lambda module, args: None)
            old_calls = calls[0]
            hook_reason = refusal(capture)
            assert calls[0] == old_calls
            handle.remove()
            placement.serve_lora("per_step")
            plain = capture()
            plain.close()
            assert calls[0] == old_calls + 2, (
                "a LoRA mode without actual factors refused plain weights"
            )
            placement.serve_lora("bake")
            with torch.no_grad():
                built.net(state)

            # A real mutation of declared module state is restored before safe fallback.
            built.net.counter = 7
            original = state.clone()

            def mutating(index):
                built.net.counter += 1
                step(index)

            state_reason = refusal(
                lambda: PreparedRepeat.capture(
                    mutating,
                    state=(state,),
                    steps=4,
                    owners=(built.net,),
                    resident_only=True,
                )
            )
            assert built.net.counter == 7
            torch.testing.assert_close(state, original, rtol=0, atol=0)
            del built.net.counter

            # Real impossible allocation after work has been queued on the warmup stream.
            def exhausted(index):
                del index
                state.add_(3)
                torch.empty(
                    2 * torch.cuda.get_device_properties(0).total_memory,
                    dtype=torch.uint8,
                    device="cuda",
                )

            try:
                PreparedRepeat.capture(
                    exhausted,
                    state=(state,),
                    steps=4,
                    owners=(built.net,),
                    resident_only=True,
                )
            except torch.OutOfMemoryError:
                pass
            else:
                raise AssertionError("impossible allocation did not refuse capture")
            torch.testing.assert_close(state, original, rtol=0, atol=0)
            with torch.no_grad():
                torch.testing.assert_close(
                    built.net(state), state * 0.5, rtol=0, atol=0
                )

            # Publish an actual ATen linear callable through the existing readiness registry.
            # This exercises an epoch transition; it is not a bake compiler timing claim.
            epoch_plan = capture()
            before_epoch = kernel_ready.epoch()
            linear = kernel_ready.wait(
                f"resident-owner-linear-{seed}", lambda wait: torch.nn.functional.linear
            )
            torch.testing.assert_close(
                linear(state, matrix.cuda()), state * 0.5, rtol=0, atol=0
            )
            assert kernel_ready.epoch() > before_epoch
            try:
                epoch_plan.submit()
            except RuntimeError as error:
                assert "kernel implementations changed" in str(error), str(error)
            else:
                raise AssertionError("stale kernel definition was submitted")
            assert epoch_plan.facts()["host_graph_launches"] == 0
            epoch_plan.close()
            lora_reason = real_lora_refusal(
                memory, Path(directory) / "lora.cozytensors", seed
            )
            return {
                "runtime": importlib.metadata.version("cozy-runtime"),
                "tensorfs": importlib.metadata.version("tensorfs"),
                "device": torch.cuda.get_device_name(0),
                "cold_no_read": cold_reason,
                "resident_read_bytes": 0,
                "phase_changes": phase_changes,
                "overlap_before_capture": before_overlap,
                "overlap_after_capture": request.overlap,
                "hook_refusal": hook_reason,
                "per_step_lora_refusal": lora_reason,
                "state_restored": state_reason,
                "real_oom_restored_and_healthy": True,
                "kernel_registry_epoch_invalidated": True,
                "host_graph_launches": 1,
            }
        finally:
            for plan in plans:
                plan.close()
            backend.close()
            memory.trim()
            checkpoint.close()


def attention_proof(seed, ctx):
    """Real zero-dropout SDPA replay; seeded dropout and random generation still refuse."""
    assert ctx.device.type == "cuda"
    accel.initialize(torch, "cuda")
    query = torch.arange(512, device="cuda", dtype=torch.float32).reshape(1, 2, 8, 32)
    query = (query / 512).to(torch.float16)
    key, value = query.clone(), query * 0.5
    with torch.inference_mode(False):
        state = torch.zeros_like(query)
    expected = torch.nn.functional.scaled_dot_product_attention(
        query, key, value, dropout_p=0.0
    )

    def step(index):
        del index
        state.copy_(
            torch.nn.functional.scaled_dot_product_attention(
                query, key, value, dropout_p=0.0
            )
        )

    plan = PreparedRepeat.capture(step, state=(state,), steps=2)
    try:
        output = ctx.plan(plan).wait()[0]
        torch.testing.assert_close(output, expected, rtol=0, atol=0)
        assert plan.facts()["host_graph_launches"] == 1
    finally:
        plan.close()

    def dropout(index):
        del index
        state.copy_(
            torch.nn.functional.scaled_dot_product_attention(
                query, key, value, dropout_p=0.25
            )
        )

    def random(index):
        del index
        state.copy_(torch.rand_like(state))

    refused = []
    saved = state.clone()
    for name, body in (("dropout", dropout), ("random", random)):
        try:
            unexpected = PreparedRepeat.capture(body, state=(state,), steps=2)
        except RuntimeError as error:
            assert "explicit noise/RNG tensor inputs" in str(error), str(error)
            refused.append(name)
        else:
            unexpected.close()
            raise AssertionError(f"seeded {name} was accepted")
        torch.testing.assert_close(state, saved, rtol=0, atol=0)
    return {
        "seed": seed,
        "mode": "attention",
        "runtime": importlib.metadata.version("cozy-runtime"),
        "tensorfs": importlib.metadata.version("tensorfs"),
        "torch": str(torch.__version__),
        "device": torch.cuda.get_device_name(0),
        "zero_dropout_host_graph_launches": 1,
        "refused": refused,
    }


def real_lora_refusal(memory, path, seed):
    """A production PEFT composition with actual stored low-rank factors, not a mode flag."""
    width, rank = 128, 2
    transform = composer(
        Graph(FORMAT, (Linear("adapted", "0", "gate", rank, rank, 1.0, "F32"),))
    )
    writer = tensorfs.Writer(path)
    component = writer.component("adapted")
    component.put("0.base_layer.weight", torch.eye(width) * 0.5)
    component.put("0.lora_A.gate.weight", torch.full((rank, width), 1 / width))
    component.put("0.lora_B.gate.weight", torch.full((width, rank), 0.25))
    checkpoint = writer.commit()
    source = Checkpoint(checkpoint)
    rows = source.rows("adapted")
    device = torch.device("cuda:0")
    facts = measure_device(torch, 0, "cuda")
    resolution = resolve(
        variant=Variant(
            "resident-lora", str(path), checkpoint.id, {"adapted": checkpoint.id}
        ),
        rows=rows,
        providers=launch_providers(),
        device=facts,
        runtime=measure_runtime(torch, importlib.metadata.version("cozy-runtime")),
        construction=ConstructionFacts("resident-lora-proof", "Adapted", ("adapted",)),
        encoded_leaves="refuse",
        dtype_name=dtype_name,
    )
    with torch.device("meta"):
        built = SimpleNamespace(
            adapted=nn.Sequential(nn.Linear(width, width, bias=False))
        )
        transform(SimpleNamespace(components={"adapted": built.adapted}))
    walked = census(built)
    backend = Backend(
        {"adapted": source},
        rows,
        torch=torch,
        plan=resolution,
        memory=memory,
        device=device,
        construction="Adapted",
        device_facts=facts,
        release="resident-lora-proof",
    )
    backend.expect({"adapted": [row.key for row in rows]})
    fit = backend.fit(walked, encoded_leaves="refuse")
    assert fit and fit["ok"], fit
    live = backend.materialize(built, walked, encoded_leaves="refuse")
    for destination in walked.destinations:
        backend.fill(destination.key, destination.spec, live[destination.key])
    backend.commit()
    placement = Placement(memory, backend)
    calls = [0]
    with torch.inference_mode(False):
        state = torch.ones((2, width), device=device)

    def step(index):
        del index
        calls[0] += 1
        state.copy_(built.adapted(state))

    try:
        placement.open_attempt()
        placement.serve_lora("per_step")
        assert placement.bakes["adapted"].bakes, "no actual LoRA factors were attached"
        with memory.request(f"real-lora-{seed}") as request:
            placement.plan(("adapted",), request)
            with torch.no_grad(), placement.using("adapted", ("adapted",)):
                result = built.adapted(state)
            torch.testing.assert_close(result, torch.ones_like(state), rtol=0, atol=0)
            try:
                PreparedRepeat.capture(
                    step,
                    state=(state,),
                    steps=4,
                    owners=(built.adapted,),
                    resident_only=True,
                )
            except CapabilityError as error:
                assert error.code == "prepared_graph_ineligible" and "LoRA" in str(
                    error
                ), str(error)
                assert calls[0] == 0, "variable LoRA reached capture warmup"
                return str(error)
            raise AssertionError(
                "actual per-step LoRA was admitted to a fixed native interval"
            )
    finally:
        placement.release()
        backend.close()
        memory.trim()
        checkpoint.close()
