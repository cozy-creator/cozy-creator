"""Real Torch conversion + production ABI refusal; no CUDA Plan is constructed.

Closed public Plan wrappers exercise Python retirement metadata. The absent-handle
case calls the actual native close refusal and verifies transactional ownership.
"""

from __future__ import annotations

import json
from pathlib import Path

import torch
from tensorfs.varena import api
from tensorfs.varena import torch as vt
from tensorfs.varena._offload import _Weight

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
    plan = api.Plan(0, None)  # Closed real public wrapper, not a native CUDA Plan.
    owner._intent_plan = plan
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


class RefuseOnce(_Weight):
    """One real Tensor .data setter failure at the publication boundary."""

    @classmethod
    def __torch_function__(cls, function, types, args=(), kwargs=None):
        if (
            getattr(function, "__name__", "") == "__set__"
            and getattr(function, "__self__", None) is torch.Tensor.data
            and args[0].fail_next_set
        ):
            args[0].fail_next_set = False
            raise RuntimeError("one-shot Tensor setter publication refusal")
        return super().__torch_function__(function, types, args, kwargs)


def rollback():
    model = torch.nn.Sequential(torch.nn.Linear(3, 3), torch.nn.Linear(3, 3)).eval()
    owner = vt.offload(model, device="cpu")
    with torch.no_grad():
        model(torch.ones(1, 3))
    parameters = list(model.parameters())
    before = [(p._version_owner, p.dtype, p.clone()) for p in parameters]
    plan = api.Plan(0, None)
    owner._intent_plan = plan
    refused_parameter = parameters[1]
    refused_parameter.__class__ = RefuseOnce
    refused_parameter.fail_next_set = True
    try:
        model.double()
    except RuntimeError as error:
        assert str(error) == "one-shot Tensor setter publication refusal"
    else:
        raise AssertionError("Tensor setter failure was swallowed")
    assert owner._intent_plan is plan and not owner.intent_facts()["pending_refresh"]
    for parameter, (version, dtype, values) in zip(parameters, before):
        assert parameter._version_owner is version and parameter.dtype == dtype
        assert torch.equal(parameter.clone(), values)
    owner._intent_plan = None  # The fixture's wrapper was already closed (handle 0).
    owner.close()


def noop():
    model, owner = enrolled()
    plan = api.Plan(2**64 - 1, None)
    owner._intent_plan = plan
    version = model.left.weight._version_owner
    try:
        model.float()
        assert owner._intent_plan is plan and plan._handle == 2**64 - 1
        assert model.left.weight._version_owner is version
        assert not owner.intent_facts()["pending_refresh"]
    finally:
        plan._handle = 0
        owner._intent_plan = None
        owner.close()


case(
    "changed dtype retires closed-wrapper metadata; ties and escaped versions survive",
    success,
)
case("real native Plan-close refusal rolls back exact versions/buffer/Plan", refusal)
case(
    "real Tensor setter failure restores versions before any Plan retirement", rollback
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
