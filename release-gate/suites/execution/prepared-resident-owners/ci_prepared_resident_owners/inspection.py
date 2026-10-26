"""Actual host-to-device graph rejection, followed by healthy native execution."""

import importlib.metadata
import re

import torch
from cozy_runtime.author import CapabilityError
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.internal import accel


def inspection_proof(seed, ctx):
    assert ctx.device.type == "cuda"
    accel.initialize(torch, "cuda")
    device = torch.cuda.current_device()
    observed_capture = []
    with torch.inference_mode(False), torch.no_grad():
        host = torch.empty(256, dtype=torch.float32, pin_memory=True)
        host.copy_(torch.arange(256, dtype=torch.float32).add(seed % 97))
        assert host.device.type == "cpu" and host.is_pinned()
        state = torch.full((256,), -19.0, dtype=torch.float32, device=device)
        saved = state.clone()

        def host_copy(index):
            del index
            observed_capture.append(torch.cuda.is_current_stream_capturing())
            state.copy_(host, non_blocking=True)

        try:
            unexpected = PreparedRepeat.capture(
                host_copy, state=(state,), steps=2, owners=(host,)
            )
        except CapabilityError as error:
            assert error.code == "prepared_graph_ineligible", str(error)
            native_error = error.__cause__
            assert native_error is not None
            assert getattr(native_error, "eligibility_refusal", False) is True
            assert not hasattr(native_error, "error_code"), str(native_error)
            message = str(native_error)
            metadata = dict(re.findall(r"\b([a-z_]+)=([^\s]+)", message))
            assert metadata.get("stage") == "source", message
            # CUDA can retain explicit H2D or report Default and a host source pointer.
            explicit_h2d = (
                metadata.get("reason") == "memcpy_layout_or_kind"
                and metadata.get("copy_kind") == "1"
            )
            default_host = (
                metadata.get("reason") == "memcpy_source"
                and metadata.get("copy_kind") == "4"
                and metadata.get("pointer_type") == "1"
            )
            assert explicit_h2d or default_host, message
            assert int(metadata["depth"]) >= 0
            assert 0 < int(metadata["local"]) <= int(metadata["total"]) <= 1_000_000
            assert int(metadata["expected_device"]) == device
        else:
            unexpected.close()
            raise AssertionError("a captured host-to-device copy was accepted")

        # Real warmup and capture ran; no rejected plan reached Context submission.
        assert observed_capture == [False, True], observed_capture
        torch.testing.assert_close(state, saved, rtol=0, atol=0)
        torch.cuda.current_stream(device).synchronize()
        torch.testing.assert_close(state + 1, saved + 1, rtol=0, atol=0)

        calls = 0

        def valid(index):
            nonlocal calls
            del index
            calls += 1
            state.add_(3)

        plan = PreparedRepeat.capture(valid, state=(state,), steps=2)
        try:
            assert calls == 2
            torch.testing.assert_close(state, saved, rtol=0, atol=0)
            execution = ctx.plan(plan)  # The request's only submitted plan.
            output = execution.wait()[0]
            progress = execution.poll()
            assert progress.done and not progress.canceled
            assert progress.completed_steps == 2
            torch.testing.assert_close(output, saved + 6, rtol=0, atol=0)
            facts = plan.facts()
            assert facts["host_graph_launches"] == 1
            assert calls == 2, "native replay called Python"
        finally:
            plan.close()

    return {
        "mode": "inspection",
        "seed": seed,
        "runtime": importlib.metadata.version("cozy-runtime"),
        "tensorfs": importlib.metadata.version("tensorfs"),
        "torch": str(torch.__version__),
        "device": torch.cuda.get_device_name(device),
        "host_source_pinned": True,
        "copy_bytes": host.numel() * host.element_size(),
        "observed_capture": observed_capture,
        "refusal": message,
        "inspection": metadata,
        "refused_before_context_submission": True,
        "state_restored": True,
        "healthy_followup_completed_steps": progress.completed_steps,
        "healthy_followup_python_calls": calls,
        "healthy_followup_native": facts,
    }
