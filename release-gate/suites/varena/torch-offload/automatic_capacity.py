"""Actual full-controller pre-body capacity selection, no mocked allocator.

GPU operator-only: native measured target stays unchanged. Native process limit
is set to observed Working backing (no weights in this real ReLU model).
CPU mode is real eager/body-error evidence, not admission/capture simulation.
"""

from __future__ import annotations

import argparse
import json
import sys

import torch
from tensorfs.varena import torch as vt


def run(args):
    model = torch.nn.ReLU().eval()
    owner = vt.offload(model, device=args.device, graphs="auto")
    body_calls = []
    code = torch.nn.ReLU.forward.__code__

    def profile(frame, event, value):
        if (
            event == "call"
            and frame.f_code is code
            and frame.f_locals.get("self") is model
        ):
            body_calls.append(True)

    previous = sys.getprofile()
    sys.setprofile(profile)
    try:
        if args.device == "cpu":
            value = torch.linspace(-1, 1, 32)
            with torch.no_grad():
                result = model(value)
            torch.testing.assert_close(result, value.clamp_min(0), rtol=0, atol=0)
            before = len(body_calls)
            try:
                model(value.to(torch.complex64))
            except RuntimeError:
                assert len(body_calls) == before + 1
            else:
                raise AssertionError("actual unsupported complex ReLU did not refuse")
            assert not torch.cuda.is_initialized()
            owner.close()
            return {
                "passed": True,
                "scope": "real CPU public eager/error path; NO native admission/graphs",
                "body_calls": len(body_calls),
                "cuda_initialized": False,
            }

        device = torch.device(args.device)
        stream = torch.cuda.Stream(device=device)
        with torch.no_grad(), torch.cuda.stream(stream):
            value = torch.linspace(-1, 1, 32, device=device)
            expected = value.clamp_min(0).cpu()
            cold = model(value)
            stream.synchronize()
            torch.testing.assert_close(cold.cpu(), expected, rtol=0, atol=0)
            row = next(iter(owner.graph_facts()["boundaries"].values()))
            assert row["cold_observations"] == 1 and row["native_peak"] > 0
            native_peak = row["native_peak"]
            capture_pool_bytes = row["capture_pool_bytes"]
            assert capture_pool_bytes >= native_peak
            del cold
            stream.synchronize()
            vt.trim(device)
            baseline = vt.stats(device)
            assert baseline["mapped_bytes"] > 0
            calls_before = len(body_calls)
            try:
                # Input anchors its real existing Working page. A new private
                # pool cannot fit; the tiny eager output can reuse that page.
                vt.set_limit(device, baseline["mapped_bytes"])
                fallback = model(value)
                stream.synchronize()
                refused = next(iter(owner.graph_facts()["boundaries"].values()))
                assert refused["capacity_refusals"] == 1
                assert refused["reason"] == "prebody_capacity_refused"
                assert refused["native_peak"] == native_peak
                assert refused["capture_pool_bytes"] == capture_pool_bytes
                assert refused["captures"] == refused["replays"] == 0
                assert refused["cache"]["has_definition"] is False
                assert len(body_calls) == calls_before + 1  # No warm/capture body.
                torch.testing.assert_close(fallback.cpu(), expected, rtol=0, atol=0)
                del fallback
            finally:
                vt.set_limit(device, None)
            # A genuinely new request after capacity restoration can capture;
            # neither the failed admission nor its eager body is re-executed.
            before_capture = len(body_calls)
            promoted = model(value)
            stream.synchronize()
            assert len(body_calls) == before_capture + 2  # Warmup + definition.
            promoted_row = next(iter(owner.graph_facts()["boundaries"].values()))
            assert promoted_row["native_peak"] == native_peak
            assert promoted_row["capture_pool_bytes"] == capture_pool_bytes
            assert promoted_row["captures"] == promoted_row["replays"] == 1
            torch.testing.assert_close(promoted.cpu(), expected, rtol=0, atol=0)
            repeated = model(value)
            stream.synchronize()
            assert len(body_calls) == before_capture + 2
            torch.testing.assert_close(repeated.cpu(), expected, rtol=0, atol=0)

            # Real authored operator refusal after strategy selection is not an
            # allocation stand-in/injection. It must not launch a second body.
            before_error = len(body_calls)
            try:
                model(value.to(torch.complex64))
            except RuntimeError:
                assert len(body_calls) == before_error + 1
            else:
                raise AssertionError("unsupported complex ReLU did not propagate")
            stream.synchronize()
            final = owner.graph_facts()
            assert not owner._intent_frames and not owner._intent_failed
        owner.close()
        return {
            "passed": True,
            "gpu_executed": True,
            "native_peak_unchanged": native_peak,
            "capture_pool_bytes_unchanged": capture_pool_bytes,
            "native_limit_for_fixture": baseline["mapped_bytes"],
            "refused": refused,
            "promoted": promoted_row,
            "final": final,
            "prebody_refusal_eager_body_once": True,
            "postbody_error_not_reexecuted": True,
        }
    finally:
        sys.setprofile(previous)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", default="cuda:0")
    options = parser.parse_args()
    torch.set_num_threads(1)
    torch.set_num_interop_threads(1)
    print(json.dumps(run(options), indent=2))
