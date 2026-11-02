"""Real nested/repeated boundary execution; --device 0 adds native GPU observation.

CPU mode verifies declaration and call lifetimes only. GPU execution belongs to the
coordinator; the native observation uses an allocation-free split menu, not a mock.
"""

from __future__ import annotations

import argparse
import inspect
import json

import torch
from tensorfs.varena import torch as vt

parser = argparse.ArgumentParser()
parser.add_argument("--device", default="cpu", help="cpu or a CUDA ordinal")
args = parser.parse_args()
device = (
    torch.device("cpu")
    if args.device == "cpu"
    else torch.device("cuda", int(args.device))
)
torch.set_num_threads(1)
torch.set_num_interop_threads(1)
torch.manual_seed(101)
trace = []


class Leaf(torch.nn.Linear):
    probe = None

    def forward(self, value):
        if self.probe is not None:
            self.probe(self)
        return super().forward(value)


class Boundary(torch.nn.Module):
    probe = None
    fail = False

    def __init__(self):
        super().__init__()
        self.layers = torch.nn.ModuleList([Leaf(3, 3), Leaf(3, 3)])

    def forward(self, value):
        if self.probe is not None:
            self.probe(self)
        for layer in self.layers:
            value = layer(value)
            if self.probe is not None:
                self.probe(self)  # The child must restore the enclosing boundary.
            if self.fail:
                raise ValueError("declared boundary failure")
        return value


class Model(torch.nn.Module):
    _no_split_modules = ("Boundary",)

    def __init__(self):
        super().__init__()
        self.blocks = torch.nn.ModuleList([Boundary(), Boundary()])

    def forward(self, value):
        for block in self.blocks:
            value = block(value)
        return value


model = Model().eval()
x = torch.randn(2, 3)
with torch.no_grad():
    expected = model(x)
signature = inspect.signature(model.blocks[0].forward)
owner = vt.offload(model, device="cpu" if device.type == "cpu" else device.index)
assert inspect.signature(model.blocks[0].forward) == signature
initial = owner.intent_facts()
assert initial["pending_refresh"] and not initial["native"]
assert initial["active"] is None and not initial["regions"]
names = {id(module): "model." + name for name, module in model.named_modules() if name}


def native_view():
    return vt.split_select(
        device.index,
        "offload-intent-observation-v1",
        [("no-allocation", [])],
        preferred="no-allocation",
        stream=torch.cuda.current_stream(device).cuda_stream,
    )


def probe(module):
    name = names[id(module)]
    facts = owner.intent_facts()
    assert facts["active"] == name, (facts, name)
    row = {"boundary": name, "native": facts["native"]}
    if device.type == "cuda":
        view = native_view()
        assert view["phase"] == name and view["phase_weight_bytes"] > 0, view
        row["generation"] = view["plan_generation"]
    trace.append(row)


for module in model.modules():
    if isinstance(module, (Boundary, Leaf)):
        module.probe = probe


def exercise():
    with torch.no_grad():
        actual = model(x.to(device))
        torch.testing.assert_close(actual.cpu(), expected)
        assert owner.intent_facts()["active"] is None
        if device.type == "cuda":
            assert native_view()["phase"] is None
        model.blocks[0].forward(
            x.to(device)
        )  # Direct forward has the same intent lifetime.
        assert owner.intent_facts()["active"] is None
        model.blocks[0].fail = True
        try:
            model(x.to(device))
        except ValueError as error:
            assert str(error) == "declared boundary failure"
        else:
            raise AssertionError("failure was swallowed")
        assert owner.intent_facts()["active"] is None
        if device.type == "cuda":
            assert native_view()["phase"] is None
        model.blocks[0].fail = False
        previous = owner._intent_plan
        model.double()
        assert owner.intent_facts()["pending_refresh"]
        actual = model(x.to(device=device, dtype=torch.float64))
        assert actual.dtype == torch.float64
        assert owner.intent_facts()["active"] is None
        if device.type == "cuda":
            assert owner._intent_plan is not previous and previous._handle == 0


if device.type == "cpu":
    exercise()
    assert not owner.intent_facts()["native"]
    assert not torch.cuda.is_initialized()
else:
    stream = torch.cuda.Stream(device=device)
    stream.wait_stream(torch.cuda.current_stream(device))
    with torch.cuda.stream(stream):
        exercise()
    stream.synchronize()
    assert owner.intent_facts()["native"]
    assert trace[-1]["generation"] > trace[0]["generation"]
facts = owner.intent_facts()
assert len(facts["regions"]) == 6 and all(
    count > 0 for count in facts["regions"].values()
)
assert all(count > 0 for count in facts["entries"].values())
owner.close()
assert owner.intent_facts()["active"] is None and not owner.intent_facts()["native"]
if device.type == "cuda":
    with torch.cuda.stream(stream):
        assert native_view()["phase"] is None
print(
    json.dumps(
        {
            "device": str(device),
            "torch": torch.__version__,
            "declared": facts,
            "observations": trace,
            "gpu_executed": device.type == "cuda",
        }
    )
)
