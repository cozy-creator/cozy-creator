"""Prepared actual packed-Plan retirement check; coordinator GPU execution only."""

from __future__ import annotations

import argparse
import gc
import json
import time

import torch
from tensorfs import _ext
from tensorfs.varena import torch as vt

parser = argparse.ArgumentParser()
parser.add_argument("--device", type=int, default=0)
args = parser.parse_args()
torch.manual_seed(17)
model = torch.nn.Sequential(torch.nn.Linear(8, 8), torch.nn.Linear(8, 8)).eval()
with torch.no_grad():
    for parameter in model.parameters():
        parameter.copy_(torch.arange(parameter.numel()).reshape(parameter.shape) / 32)
owner = vt.offload(model, device=args.device)
gpu = owner._owner.gpu
stream = torch.cuda.Stream(device=args.device)
trace = {}


def current_handles():
    return sorted(
        {
            p._version_owner.region._handle
            for p in model.parameters()
            if p._version_owner.region is not None
        }
    )


def inventory(handles=()):
    return _ext._varena_public_residency_snapshot(gpu._handle, list(handles))


def entries():
    return {(a["id"], e["id"]): e for a in inventory()["arenas"] for e in a["entries"]}


def state(ids):
    observed = entries()
    return {str(key): observed.get(key) for key in ids}


def await_retired(ids):
    deadline = time.monotonic() + 30
    while True:
        observed = entries()
        if all(
            key not in observed or observed[key]["contents"] == "retired" for key in ids
        ):
            return
        if time.monotonic() >= deadline:
            raise AssertionError(state(ids))
        time.sleep(0.01)


with torch.inference_mode(), torch.cuda.stream(stream):
    inputs = torch.ones(1, 8, device=args.device)
    expected = model(inputs)
    stream.synchronize()
    old_plan = owner._intent_plan
    old = inventory(current_handles())
    old_ids = {(row["arena_id"], row["region_id"]) for row in old["region_handles"]}
    assert len(old_ids) == 2
    identities = [id(p) for p in model.parameters()]
    managed_alias = model[0].weight.detach()
    preserved = managed_alias.clone()
    native_alias = managed_alias._version_owner.tensor(
        managed_alias._shape, managed_alias._strides, managed_alias._offset, stream
    )
    native_alias.record_stream(stream)
    link = inventory([managed_alias._version_owner.region._handle])["region_handles"][0]
    retained = (link["arena_id"], link["region_id"])
    trace["before_conversion"] = state(old_ids)
    model.half()
    assert old_plan._handle == 0 and owner._intent_plan is None
    assert owner.intent_facts()["pending_refresh"]
    assert [id(p) for p in model.parameters()] == identities
    gc.collect()
    stream.synchronize()
    await_retired(old_ids - {retained})
    trace["after_conversion"] = state(old_ids)
    assert (
        entries()[retained]["pins"] > 0 and entries()[retained]["contents"] == "ready"
    )
    released = gpu.trim()  # Test control, not product behavior or a new trim policy.
    assert released > 0
    assert all(
        key not in entries() or not entries()[key]["mapped_bytes"]
        for key in old_ids - {retained}
    )
    assert entries()[retained]["mapped_bytes"] > 0
    torch.testing.assert_close(native_alias, preserved)
    assert torch.equal(managed_alias.clone(), preserved)
    stream.synchronize()
    del native_alias
    gc.collect()
    stream.synchronize()
    gpu.trim()
    assert not entries()[retained]["mapped_bytes"]
    # The legitimate managed old dtype alias can refill after physical eviction.
    assert torch.equal(managed_alias.clone(), preserved)
    with torch.no_grad():
        actual = model(inputs.half())
    stream.synchronize()
    torch.testing.assert_close(actual.float(), expected, rtol=2e-3, atol=2e-3)
    assert (
        owner._intent_plan is not None and not owner.intent_facts()["pending_refresh"]
    )
    trace["after_refill"] = state(old_ids)
    del managed_alias
    gc.collect()
    stream.synchronize()
    await_retired(old_ids)
    gpu.trim()
    trace["after_alias_drop"] = state(old_ids)
owner.close()
print(
    json.dumps(
        {
            "gpu_executed": True,
            "torch": torch.__version__,
            "automatic_stale_plan_close": True,
            "unescaped_retirement": True,
            "native_alias_pin_preserved": True,
            "managed_alias_old_dtype_refill": True,
            "new_intent_rebuilt": True,
            "trace": trace,
        }
    )
)
