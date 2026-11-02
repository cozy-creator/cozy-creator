"""Actual Torch CPU model inventory contract; no native residency or CUDA proof."""

from __future__ import annotations

import gc
import importlib.util
import json
import sys
import weakref
from pathlib import Path

import torch
from torch import nn

ROOT = Path(__file__).resolve().parents[4]
SOURCE = ROOT / "crates/tensorfs-py/python/tensorfs/varena/_offload_inventory.py"
spec = importlib.util.spec_from_file_location("offload_inventory_under_review", SOURCE)
inventory = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = inventory
spec.loader.exec_module(inventory)


def refused(module, message):
    before = [
        (name, id(p), p.detach().clone(), p.requires_grad)
        for name, p in module.named_parameters()
    ]
    try:
        inventory.inspect_cpu_roots({"model": module})
    except inventory.InventoryError as error:
        assert message in str(error), str(error)
    else:
        raise AssertionError("unsupported state was accepted")
    after = list(module.named_parameters())
    assert [(n, id(p), p.requires_grad) for n, p in after] == [
        (n, i, g) for n, i, _, g in before
    ]
    for (_, p), (_, _, value, _) in zip(after, before):
        torch.testing.assert_close(p, value, rtol=0, atol=0)


def main():
    assert torch.version.cuda is None and not torch.cuda.is_initialized()
    torch.set_num_threads(1)
    torch.manual_seed(7)
    model = nn.Module()
    model.left = nn.Linear(4, 4, bias=False)
    model.right = nn.Linear(4, 4, bias=False)
    model.right.weight = model.left.weight
    model.reused = model.left
    model.layers = nn.ModuleList([nn.Linear(4, 4), nn.Linear(4, 4)])
    model.register_buffer("state", torch.arange(4.0))
    model.eval()
    original = model.left.weight
    value = torch.arange(4.0)
    with torch.no_grad():
        expected = model.left(value) + model.right(value)
    result = inventory.inspect_cpu_roots({"model": model})
    tied = [p for p in result.parameters if p.tensor is original]
    assert len(tied) == 1 and len(tied[0].slots) == 2
    assert {path for slot in tied[0].slots for path in slot.paths} == {
        "model.left.weight",
        "model.right.weight",
        "model.reused.weight",
    }
    assert result.boundaries == ("model.layers.0", "model.layers.1")
    assert model.left.weight is model.right.weight is original
    torch.testing.assert_close(
        model.left(value) + model.right(value), expected, rtol=0, atol=0
    )
    del expected
    gc.collect()

    views = nn.Module()
    base = torch.arange(24.0).reshape(4, 6)
    views.a = nn.Parameter(base, requires_grad=False)
    views.b = nn.Parameter(base[1:, 1:].t(), requires_grad=False)
    views.empty = nn.Parameter(torch.empty(0), requires_grad=False)
    views.eval()
    description = inventory.inspect_cpu_roots({"view": views})
    assert len(description.parameters) == 3 and len(description.storages) == 2
    b = next(p for p in description.parameters if p.tensor is views.b)
    assert b.stride == views.b.stride() and b.byte_offset == 28 and b.byte_end == 96
    assert (
        len(next(s for s in description.storages if len(s.parameters) == 2).parameters)
        == 2
    )

    class Block(nn.Linear):
        pass

    class DerivedBlock(Block):
        pass

    left, right = nn.Module(), nn.Module()
    left._no_split_modules = ["Block"]
    left.child = DerivedBlock(2, 2)
    right.child = Block(2, 2)
    scoped = inventory.inspect_cpu_roots({"left": left.eval(), "right": right.eval()})
    assert scoped.boundaries == ("left.child", "right")
    fallback = inventory.inspect_cpu_roots(
        {
            "layers": nn.Sequential(nn.Linear(2, 2), nn.Linear(2, 2)).eval(),
            "vae": nn.Linear(2, 2).eval(),
        }
    )
    assert fallback.boundaries == ("layers.0", "layers.1", "vae")

    refused(nn.Linear(2, 2), "eval()")
    bad = nn.Linear(2, 2).eval()
    ref = weakref.ref(bad.weight)
    refused(bad, "weak references")
    assert ref() is bad.weight
    del ref
    bad.weight.grad = torch.ones_like(bad.weight)
    refused(bad, "gradient state")
    saved_owner = nn.Module().eval()
    saved_owner.weight = nn.Parameter(torch.ones(2, 2), requires_grad=False)
    saved_graph = torch.ones(2, 2, requires_grad=True) @ saved_owner.weight
    assert saved_owner.weight._use_count() == 2
    refused(saved_owner, "outstanding TensorImpl")
    del saved_graph
    exported = nn.Linear(2, 2, bias=False).eval()
    capsule = torch.utils.dlpack.to_dlpack(exported.weight)
    assert exported.weight._use_count() == 2
    refused(exported, "outstanding TensorImpl")
    del capsule
    conjugate = nn.Module().eval()
    conjugate.weight = nn.Parameter(torch.tensor([1 + 2j]).conj(), requires_grad=False)
    refused(conjugate, "conjugate/negative")
    mixed = nn.Module()
    mixed.weight = nn.Parameter(torch.ones(4), requires_grad=False)
    mixed.register_buffer("mutable", mixed.weight.detach())
    mixed.eval()
    refused(mixed, "mutable buffer storage")
    hooked = nn.Linear(2, 2).eval()
    hooked._hf_hook = object()
    refused(hooked, "existing Accelerate")
    meta = nn.Linear(2, 2, device="meta").eval()
    try:
        inventory.inspect_cpu_roots({"meta": meta})
    except inventory.InventoryError as error:
        assert "materialized CPU" in str(error)
    else:
        raise AssertionError("unbacked meta weights accepted")
    cycle = nn.Module().eval()
    cycle.child = cycle
    try:
        inventory.inspect_cpu_roots({"cycle": cycle})
    except inventory.InventoryError as error:
        assert "module cycle" in str(error)
    else:
        raise AssertionError("module cycle accepted")
    assert not torch.cuda.is_initialized()
    assert not any(
        "libcuda.so" in line
        for line in Path("/proc/self/maps").read_text().splitlines()
    )
    print(
        json.dumps(
            {
                "torch": torch.__version__,
                "cuda": False,
                "inventory": result.facts(),
                "storage_views": description.facts(),
                "unchanged_real_cpu_forward": True,
                "refusal_preserves_model": True,
                "offload_implemented": False,
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
