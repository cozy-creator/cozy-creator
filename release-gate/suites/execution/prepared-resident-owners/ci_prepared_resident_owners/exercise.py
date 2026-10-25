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
            with memory.request(
                f"resident-owner-{seed}",
                plan=[backend.models["net"], backend.models["next"]],
            ) as request:
                cold_reason = refusal(capture)
                assert calls[0] == 0 and request.moved == 0, request.stats()
                with torch.no_grad():
                    torch.testing.assert_close(
                        built.net(state), state * 0.5, rtol=0, atol=0
                    )
                before = request.moved
                plan = capture()
                assert calls[0] == 2 and request.moved == before
                assert not request.overlap, "idle capture changed real request overlap"
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
                assert not request.overlap
                phase_changes = request.phase_changes

            # Supported ownership rejects Python hooks and variable per-step LoRA before warmup.
            handle = built.net.register_forward_pre_hook(lambda module, args: None)
            old_calls = calls[0]
            hook_reason = refusal(capture)
            assert calls[0] == old_calls
            handle.remove()
            placement.serve_lora("per_step")
            lora_reason = refusal(capture)
            assert calls[0] == old_calls
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
            return {
                "runtime": importlib.metadata.version("cozy-runtime"),
                "tensorfs": importlib.metadata.version("tensorfs"),
                "device": torch.cuda.get_device_name(0),
                "cold_no_read": cold_reason,
                "resident_read_bytes": 0,
                "phase_changes": phase_changes,
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
