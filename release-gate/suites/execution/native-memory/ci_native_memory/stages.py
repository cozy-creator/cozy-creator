"""Real two-stage GPU data edges, native advancement and request ownership."""

from __future__ import annotations

import gc
import json
import os
import tempfile
import time
from contextlib import ExitStack
from pathlib import Path

import msgspec
from cozy_runtime.author import Context, Telemetry


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "native"
    seed: int = 3265001


class Report(msgspec.Struct):
    passed: bool
    detail: str


def emit(tel, event, **fields):
    value = "SEQUENCE_GATE " + json.dumps(
        {"event": event, **fields}, separators=(",", ":")
    )
    assert len(value) <= 190
    tel.log(value)


def gpu(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    import torch
    from cozy_runtime.compute import PreparedRepeat, PreparedSequence

    mode = payload.mode
    assert mode in {
        "native",
        "reuse",
        "ordered",
        "cancel",
        "stale",
        "cancel-next",
        "ownership",
        "unsupported",
    }
    emit(tel, "start", pid=os.getpid(), mode=mode, seed=payload.seed)
    device = torch.device(str(ctx.device))
    torch.set_num_threads(1)
    calls = [0, 0]
    first_steps, second_steps = (
        (1_000_000 if mode in {"cancel", "cancel-next"} else 11),
        7,
    )
    with torch.inference_mode(False):
        parent = torch.arange(256, dtype=torch.float32, device=device)
        untouched = parent.clone()
        program_input = parent.clone()
        boundary = torch.zeros_like(parent)
        final = torch.zeros_like(parent)

    def first(index):
        calls[0] += 1
        boundary.copy_(program_input + index.reshape(()).to(torch.float32))

    def second(index):
        calls[1] += 1
        final.copy_(boundary * 3 + index.reshape(()).to(torch.float32))

    a = PreparedRepeat.capture(
        first, state=(boundary,), steps=first_steps, owners=(program_input,)
    )
    b = PreparedRepeat.capture(
        second, state=(final,), steps=second_steps, owners=(boundary,)
    )
    assert calls == [2, 2]
    assert bool((boundary == 0).all()) and bool((final == 0).all())
    plan = None
    resources = ExitStack()
    unrelated = loaded = None
    try:
        if mode == "stale":
            with torch.inference_mode(False):
                independent_state = torch.zeros_like(parent)

            def independent(index):
                independent_state.copy_(
                    program_input + index.reshape(()).to(torch.float32) + 99
                )

            following = PreparedRepeat.capture(
                independent,
                state=(independent_state,),
                steps=1,
                owners=(program_input,),
            )
            resources.callback(following.close)
            boundary.resize_(128)
            assert following.valid(), (
                "the fail-fast follower must be independently eligible"
            )
            failures = []
            for stage in (a, following):
                try:
                    ctx.plan(stage)
                except RuntimeError as error:
                    failures.append(str(error))
                else:
                    raise AssertionError(
                        "a later provider ran after predecessor failure"
                    )
            assert len(failures) == 2
            assert following.valid()
            assert (
                a.facts()["host_graph_launches"]
                == following.facts()["host_graph_launches"]
                == 0
            )
            emit(
                tel,
                "refused",
                zero_launches=True,
                failed_stages=2,
                next_definition_valid=True,
                parent_unchanged=torch.equal(parent, untouched),
            )
            return Report(
                True,
                json.dumps({"pid": os.getpid(), "mode": mode, "failures": failures}),
            )
        if mode == "ordered":
            first_execution = ctx.plan(a)
            # Deliberately no explicit wait: request admission must join its predecessor.
            second_execution = ctx.plan(b)
            result = second_execution.wait()[0]
            assert first_execution.completion_proven()
            facts = {
                "host_graph_launches": a.facts()["host_graph_launches"]
                + b.facts()["host_graph_launches"],
                "native_stage_advancement": False,
                "scope": "two Python submissions ordered by request",
            }
        else:
            plan = PreparedSequence((a, b))
            if mode in {"ownership", "unsupported"}:
                with torch.inference_mode(False):
                    extra_state = torch.zeros_like(parent)

                def extra(index):
                    extra_state.copy_(parent + index.reshape(()).to(torch.float32) + 5)

                unrelated = PreparedRepeat.capture(
                    extra, state=(extra_state,), steps=1, owners=(parent,)
                )
                resources.callback(unrelated.close)
                try:
                    PreparedSequence((unrelated, a))
                except RuntimeError as error:
                    assert "another sequence" in str(error)
                else:
                    raise AssertionError("a stage acquired two sequence owners")
                gc.collect()
                assert plan.valid() and unrelated.valid(), (
                    "a rejected constructor closed another definition"
                )
                if mode == "unsupported":
                    import tensorfs
                    from cozy_runtime.author import CapabilityError
                    from tensorfs import varena

                    folder = Path(
                        resources.enter_context(
                            tempfile.TemporaryDirectory(prefix="cozy-sequence-source-")
                        )
                    )
                    writer = tensorfs.Writer(folder / "weights.cozytensors")
                    writer.component("net").put(
                        "weight", torch.ones(4, dtype=torch.float32)
                    )
                    checkpoint = writer.commit()
                    resources.callback(checkpoint.close)
                    slot = varena.LoadSlot(
                        varena.Arena(device.index or 0),
                        checkpoint,
                        [("net/weight#value", 0, 16)],
                        bytes=16,
                    )
                    resources.callback(slot.close)
                    weight = slot.tensor.view(torch.float32)
                    with torch.inference_mode(False):
                        loaded_state = torch.zeros(1, device=device)

                    def loaded_body(index):
                        del index
                        loaded_state.copy_(weight.sum())

                    loaded = PreparedRepeat.capture(
                        loaded_body, state=(loaded_state,), steps=1, load=slot
                    )
                    resources.callback(loaded.close)
                    try:
                        PreparedSequence((unrelated, loaded))
                    except CapabilityError as error:
                        assert error.code == "prepared_load_unavailable"
                    else:
                        raise AssertionError(
                            "one-shot immediate loader was admitted as deferred"
                        )
                    gc.collect()
                    assert plan.valid() and unrelated.valid() and loaded.valid()
            execution = ctx.plan(plan)
            if mode in {"cancel", "cancel-next"}:
                deadline = time.monotonic() + 10
                while execution.poll().completed_steps == 0:
                    assert time.monotonic() < deadline
                    time.sleep(0.001)
                execution.cancel()
                execution.wait()
                progress = execution.poll()
                assert progress.canceled and progress.completed_steps < first_steps
                facts = execution.facts()
                assert facts["sequence"]["completion_proven"]
                assert facts["sequence"]["completed_stages"] == 0
                assert b.facts()["host_graph_launches"] == 0
                assert bool((final == 0).all())
                if mode == "cancel-next":
                    with torch.inference_mode(False):
                        extra_state = final.clone()

                    def extra(index):
                        extra_state.add_(index.reshape(()).to(torch.float32) + 1)

                    extra_plan = PreparedRepeat.capture(
                        extra, state=(extra_state,), steps=3
                    )
                    try:
                        try:
                            ctx.plan(extra_plan)
                        except Exception as error:  # noqa: BLE001 - inspect exact real failure before rethrow
                            assert "preceding prepared stage was canceled" in str(error)
                        else:
                            raise AssertionError(
                                "later stage started after cancellation"
                            )
                        assert extra_plan.facts()["host_graph_launches"] == 0
                        emit(
                            tel,
                            "refused",
                            canceled_predecessor=True,
                            zero_later_launches=True,
                        )
                    finally:
                        extra_plan.close()
                detail = {
                    "pid": os.getpid(),
                    "mode": mode,
                    "native": facts,
                    "python_body_calls": calls,
                    "parent_unchanged": torch.equal(parent, untouched),
                }
                return Report(True, json.dumps(detail, sort_keys=True))
            # No Python polling/callback drives either stage. Observe native completion
            # before wait triggers the single final output copy/readout.
            time.sleep(0.25)
            deadline = time.monotonic() + 10
            facts = execution.facts()
            while not facts["sequence"]["done"]:
                assert time.monotonic() < deadline, facts
                time.sleep(0.05)
                # Host-only facts do not poll the GPU or advance the native worker.
                facts = execution.facts()
            assert facts["sequence"]["completed_stages"] == 2
            assert facts["sequence"]["completion_proven"]
            assert facts["host_graph_launches"] == 2
            result = execution.wait()[0]
        expected = parent * 3 + 3 * (first_steps - 1) + (second_steps - 1)
        assert torch.equal(result, expected), float((result - expected).abs().max())
        assert torch.equal(parent, untouched) and calls == [2, 2]
        assert result.data_ptr() not in {
            parent.data_ptr(),
            boundary.data_ptr(),
            final.data_ptr(),
        }
        first_result = result.clone()
        if unrelated is not None:
            assert torch.equal(ctx.plan(unrelated).wait()[0], parent + 5)
        if loaded is not None:
            assert ctx.plan(loaded).wait()[0].item() == 4.0
        if mode == "reuse":
            retained_owners = [
                stage["retained_owner_count"] for stage in facts["stages"]
            ]
            program_input.add_(10)
            next_execution = ctx.plan(plan)
            result_again = next_execution.wait()[0]
            assert torch.equal(result_again, expected + 30)
            assert torch.equal(result, first_result), (
                "later submission overwrote earlier output"
            )
            facts = next_execution.facts()
            assert facts["host_graph_launches"] == 4
            assert facts["sequence"]["completed_stages"] == 2
            assert [
                stage["retained_owner_count"] for stage in facts["stages"]
            ] == retained_owners
            assert calls == [2, 2]
        detail = {
            "pid": os.getpid(),
            "mode": mode,
            "native": facts,
            "python_body_calls": calls,
            "parent_unchanged": True,
            "max_abs_error": float((result - expected).abs().max()),
            "edge": {
                "first_output": boundary.data_ptr(),
                "second_input": boundary.data_ptr(),
            },
            "scope": "actual two-stage dataflow; no complete-model or performance claim",
        }
        return Report(True, json.dumps(detail, sort_keys=True))
    finally:
        resources.close()
        if plan is not None:
            plan.close()
        else:
            a.close()
            b.close()
