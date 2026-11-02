"""Real Torch conversion + production ABI refusal; no CUDA Plan is constructed.

The absent-handle case calls the actual native close refusal. Other cells execute
ordinary CPU inference/conversion; no injected setter or substitute Plan success.
"""

from __future__ import annotations

import json
from pathlib import Path

import torch
from tensorfs.varena import api
from tensorfs.varena import torch as vt

torch.set_num_threads(1)
torch.set_num_interop_threads(1)
results = []
native_refusal_observed = False


class Tied(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.left = torch.nn.Linear(3, 2, bias=False)
        self.right = torch.nn.Linear(3, 2, bias=False)
        self.right.weight = self.left.weight
        self.register_buffer("state", torch.tensor(1.0))

    def forward(self, value):
        return self.left(value) + self.right(value) + self.state


def enrolled():
    model = Tied().eval()
    owner = vt.offload(model, device="cpu")
    with torch.no_grad():
        model(torch.ones(1, 3))
    assert not owner.intent_facts()["pending_refresh"]
    return model, owner


def case(name, function):
    try:
        function()
    except (AssertionError, RuntimeError, ValueError, TypeError) as error:
        results.append({"name": name, "pass": False, "error": str(error)})
    else:
        results.append({"name": name, "pass": True})


def success():
    model, owner = enrolled()
    parameter = model.left.weight
    identity = id(parameter)
    escaped = parameter.detach()
    value = escaped.clone()
    model.double()
    assert owner._intent_plan is None and owner.intent_facts()["pending_refresh"]
    assert parameter is model.right.weight and id(parameter) == identity
    assert parameter.dtype == model.state.dtype == torch.float64
    assert escaped.dtype == torch.float32 and torch.equal(escaped.clone(), value)
    with torch.no_grad():
        model(torch.ones(1, 3, dtype=torch.float64))
    assert not owner.intent_facts()["pending_refresh"]
    owner.close()
    assert torch.equal(escaped.clone(), value)


def refusal():
    global native_refusal_observed
    model, owner = enrolled()
    parameter = model.left.weight
    version = parameter._version_owner
    old_buffer = model.state
    value = parameter.clone()
    plan = api.Plan(2**64 - 1, None)  # Actual absent handle, no simulated native owner.
    owner._intent_plan = plan
    try:
        try:
            model.double()
        except api.Invalid as error:
            native_refusal_observed = True
            assert "invalid or closed handle" in str(error), str(error)
        else:
            raise AssertionError(
                "conversion committed despite real native close refusal"
            )
        assert owner._intent_plan is plan and plan._handle == 2**64 - 1
        assert not owner.intent_facts()["pending_refresh"]
        assert parameter._version_owner is version and model.right.weight is parameter
        assert parameter.dtype == torch.float32 and torch.equal(
            parameter.clone(), value
        )
        assert model.state is old_buffer and model.state.dtype == torch.float32
    finally:
        # No actual handle was created; neutralize only this deliberately absent ID.
        plan._handle = 0
        owner._intent_plan = None
        owner.close()


def conversion_refusal():
    model, owner = enrolled()
    parameters = list(model.parameters())
    before = [(p._version_owner, p.dtype, p.clone()) for p in parameters]
    buffer = model.state
    try:
        model.to(dtype=torch.int32)
    except TypeError as error:
        assert str(error) == "Module.to requires a floating or complex dtype"
    else:
        raise AssertionError("public unsupported dtype conversion succeeded")
    assert not owner.intent_facts()["pending_refresh"] and model.state is buffer
    for parameter, (version, dtype, values) in zip(parameters, before):
        assert parameter._version_owner is version and parameter.dtype == dtype
        assert torch.equal(parameter.clone(), values)
    owner.close()


def noop():
    model, owner = enrolled()
    version = model.left.weight._version_owner
    model.float()
    assert model.left.weight._version_owner is version
    assert not owner.intent_facts()["pending_refresh"]
    owner.close()


case(
    "actual CPU conversion preserves ties, escaped versions and rebuilt intent",
    success,
)
case("real native Plan-close refusal rolls back exact versions/buffer/Plan", refusal)
case(
    "public unsupported conversion preserves identities/versions/buffers",
    conversion_refusal,
)
case("no-op dtype conversion preserves existing intent without closing Plan", noop)
assert not torch.cuda.is_initialized()
assert not any(
    "libcuda.so" in line for line in Path("/proc/self/maps").read_text().splitlines()
)
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "cuda": False,
            "native_close_refusal_executed": native_refusal_observed,
            "native_cuda_plan_retirement": "not exercised",
            "results": results,
        }
    ),
    flush=True,
)
assert all(row["pass"] for row in results)
