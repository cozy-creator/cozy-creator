"""Prepared actual graph frontend fixture; coordinator GPU execution only.

Requires the proposed owner.capture API. No mocks or downloaded models. This client
is unrun; source/syntax checks do not qualify CUDA or streamed graph memory savings.
"""

from __future__ import annotations

import argparse
import gc
import json

import torch
from tensorfs import _ext
from tensorfs.varena import api
from tensorfs.varena import torch as vt
from tensorfs.varena._offload import Offload
from tensorfs.varena._offload_graph import GraphInvalidated

parser = argparse.ArgumentParser()
parser.add_argument("--device", type=int, default=0)
args = parser.parse_args()
if getattr(Offload, "capture", None) is None:
    raise NotImplementedError(
        "graph frontend is pending; run only after adapter integration"
    )
if not hasattr(api.Use, "wait_ready"):
    raise NotImplementedError("graph frontend requires scoped native Use.wait_ready")
torch.manual_seed(17)
model = torch.nn.Sequential(torch.nn.Linear(32, 32), torch.nn.Linear(32, 32)).eval()
controls = [
    (block.weight.detach().clone(), block.bias.detach().clone()) for block in model
]
owner = vt.offload(model, device=args.device)
gpu = owner._owner.gpu
stream = torch.cuda.Stream(device=args.device)
other = torch.cuda.Stream(device=args.device)
segments = []
observations = []


def observe(segment):
    facts = segment.facts()
    handles = facts["region_handles"]
    snapshot = _ext._varena_public_residency_snapshot(gpu._handle, handles)
    linked = {(row["arena_id"], row["region_id"]) for row in snapshot["region_handles"]}
    entries = {
        (arena["id"], entry["id"]): entry
        for arena in snapshot["arenas"]
        for entry in arena["entries"]
        if (arena["id"], entry["id"]) in linked
    }
    assert len(entries) == len(set(handles)) > 0
    addresses = [api._new("region_ptr", handle) for handle in handles]
    return {"facts": facts, "addresses": addresses, "entries": list(entries.values())}


with torch.inference_mode(), torch.cuda.stream(stream):
    example = torch.ones(1, 32, device=args.device)
    for block in model:
        segments.append(
            owner.capture(
                block, (example,), kwargs={}, stream=stream, reserve_bytes=2 << 20
            )
        )
    stream.synchronize()
    definitions = [observe(segment) for segment in segments]
    assert all(row["pins"] == 0 for item in definitions for row in item["entries"]), (
        "cached definitions retain weight Pins; this is resident graph support"
    )

    # An ordinary native escaped alias is an independent physical-owner control.
    control = segments[0]._definition.regions[0]
    escaped = control.tensor(dtype="uint8", shape=(32,), stream=stream.cuda_stream)
    escaped_view = escaped[1:]
    escaped_expected = escaped_view.cpu().clone()
    pinned = _ext._varena_public_residency_snapshot(gpu._handle, [control._handle])[
        "region_handles"
    ][0]["region_id"]
    del escaped
    stream.synchronize()
    gpu.trim()
    observations.append([observe(segment) for segment in segments])
    for item in observations[-1]:
        for row in item["entries"]:
            assert (
                row["mapped_bytes"] > 0
                if row["id"] == pinned
                else row["mapped_bytes"] == 0
            )
    assert torch.equal(escaped_view.cpu(), escaped_expected)
    del escaped_view
    gc.collect()
    stream.synchronize()
    gpu.trim()

    old_outputs = []
    for cycle in range(3):
        host_input = torch.full((1, 32), float(cycle + 1))
        inputs = host_input.to(device=args.device)
        result = inputs
        expected = host_input
        for index, segment in enumerate(segments):
            result = segment(result, stream=stream)
            expected = torch.nn.functional.linear(expected, *controls[index])
        stream.synchronize()
        torch.testing.assert_close(result.cpu(), expected)
        old_outputs.append((result, expected))
        for previous, control_output in old_outputs:
            torch.testing.assert_close(previous.cpu(), control_output)
        gpu.trim()
        current = [observe(segment) for segment in segments]
        assert all(
            item["addresses"] == prior["addresses"]
            for item, prior in zip(current, definitions)
        )
        assert all(
            row["pins"] == 0 and row["mapped_bytes"] == 0
            for item in current
            for row in item["entries"]
        )
        observations.append(current)

    # Definition weights stay bound to their immutable original versions.
    pending_old = segments[1](example, stream=stream)
    model.double()
    stream.synchronize()
    other.wait_stream(stream)
    example.record_stream(other)
    with torch.cuda.stream(other):
        try:
            segments[0](example, stream=other)
        except GraphInvalidated:
            pass
        else:
            raise AssertionError("stale dtype definition admitted a new replay")
    other.synchronize()
    torch.testing.assert_close(
        pending_old.cpu(), torch.nn.functional.linear(torch.ones(1, 32), *controls[1])
    )
    assert (
        segments[0].facts()["version_dtypes"]
        == definitions[0]["facts"]["version_dtypes"]
    )
    stream.synchronize()
    other.synchronize()
    for segment in segments:
        segment.close()
        assert segment.facts()["closed"]
owner.close()
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "gpu_executed": True,
            "definition_weight_pins_zero": True,
            "same_va_trim_refill_cycles": 3,
            "independent_outputs": True,
            "old_dtype_definitions": True,
            "observations": observations,
        }
    )
)
