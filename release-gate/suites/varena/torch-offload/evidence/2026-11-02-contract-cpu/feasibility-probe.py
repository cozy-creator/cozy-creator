"""Real CPU Torch dispatcher/identity probe; not a Varena allocator mock/proof."""
from __future__ import annotations

import contextvars
import json
import traceback
import weakref
from contextlib import contextmanager
from pathlib import Path

import torch
from torch.utils._python_dispatch import return_and_correct_aliasing
from torch.utils._pytree import tree_flatten, tree_map

torch.set_num_threads(1)
torch.set_num_interop_threads(1)
assert torch.__version__ == "2.14.1+cpu"
assert torch.version.cuda is None and not torch.cuda.is_initialized()
assert not any("libcuda.so" in line for line in Path("/proc/self/maps").read_text().splitlines())

CONVERT = contextvars.ContextVar("controlled_module_conversion", default=False)
OPS = []


class Weight(torch.Tensor):
    __torch_function__ = torch._C._disabled_torch_function_impl

    @staticmethod
    def __new__(cls, value, *, family="original"):
        return torch.Tensor._make_wrapper_subclass(
            cls, value.shape, strides=value.stride(), storage_offset=value.storage_offset(),
            dtype=value.dtype, layout=value.layout, device=value.device,
            requires_grad=False,
        )

    def __init__(self, value, *, family="original"):
        self.elem = value
        self.family = family

    def __tensor_flatten__(self):
        return ["elem"], self.family

    @staticmethod
    def __tensor_unflatten__(inner, family, outer_size, outer_stride):
        return Weight(inner["elem"], family=family)

    @classmethod
    def __torch_dispatch__(cls, func, types, args=(), kwargs=None):
        kwargs = kwargs or {}
        OPS.append(str(func))
        for index, argument in enumerate(func._schema.arguments):
            value = args[index] if index < len(args) else kwargs.get(argument.name)
            leaves = tree_flatten(value)[0]
            if argument.alias_info is not None and argument.alias_info.is_write:
                if any(isinstance(leaf, Weight) for leaf in leaves):
                    raise RuntimeError(f"immutable managed argument: {func}/{argument.name}")
        weights = [value for value in tree_flatten((args, kwargs))[0] if isinstance(value, Weight)]
        unwrap = lambda value: value.elem if isinstance(value, Weight) else value
        result = func(*tree_map(unwrap, args), **tree_map(unwrap, kwargs))

        def wrap(value):
            if not isinstance(value, torch.Tensor):
                return value
            for weight in weights:
                if torch._C._is_alias_of(value, weight.elem):
                    return Weight(value, family=weight.family)
            if CONVERT.get() and func is torch.ops.aten._to_copy.default:
                return Weight(value, family=f"cast:{value.dtype}")
            return value

        return return_and_correct_aliasing(func, args, kwargs, tree_map(wrap, result))


@contextmanager
def conversion():
    token = CONVERT.set(True)
    try:
        yield
    finally:
        CONVERT.reset(token)


class Tied(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.left = torch.nn.Linear(3, 2, bias=False)
        self.right = torch.nn.Linear(3, 2, bias=False)
        self.left.weight = torch.nn.Parameter(torch.arange(6.0).reshape(2, 3), requires_grad=False)
        self.right.weight = self.left.weight

    def forward(self, value):
        return self.left(value) + self.right(value)


def installed():
    module = Tied().eval()
    original = module.left.weight
    original_id = id(original)
    managed = torch.nn.Parameter(Weight(original.detach().clone()), requires_grad=False)
    torch.utils.swap_tensors(original, managed)
    assert id(module.left.weight) == id(module.right.weight) == original_id
    assert isinstance(original, Weight) and isinstance(original, torch.nn.Parameter)
    return module, original


RESULTS = []


def case(name, fn):
    start = len(OPS)
    try:
        detail = fn()
        RESULTS.append({"name": name, "outcome": "pass", "detail": detail, "ops": OPS[start:]})
    except Exception as error:
        RESULTS.append({"name": name, "outcome": "observed_failure", "error": str(error), "traceback": traceback.format_exc(), "ops": OPS[start:]})


def identity_forward():
    module, parameter = installed()
    inputs = torch.arange(3.0).reshape(1, 3)
    expected = 2 * torch.nn.functional.linear(inputs, torch.arange(6.0).reshape(2, 3))
    with torch.no_grad():
        actual = module(inputs)
        direct = torch.nn.functional.linear(inputs, parameter)
    assert type(actual) is torch.Tensor and torch.equal(actual, expected)
    assert torch.equal(direct, expected / 2)
    return {"same_parameter": module.left.weight is module.right.weight, "output": actual.tolist(), "outside_reference_preserved": parameter is module.left.weight, "named_default": len(list(module.named_parameters())), "named_with_duplicates": len(list(module.named_parameters(remove_duplicate=False)))}


def aliases():
    module, parameter = installed()
    alias = parameter.transpose(0, 1).narrow(0, 1, 2).detach()
    assert isinstance(alias, Weight)
    expected = parameter.elem.transpose(0, 1).narrow(0, 1, 2).detach()
    assert alias.shape == expected.shape and alias.stride() == expected.stride()
    assert alias.storage_offset() == expected.storage_offset()
    assert torch._C._is_alias_of(alias, parameter)
    del module, parameter
    assert torch.equal(alias + 0, expected)
    return {"shape": list(alias.shape), "stride": list(alias.stride()), "offset": alias.storage_offset(), "family": alias.family, "after_module_drop": True}


def mutations():
    _, parameter = installed()
    before = parameter.elem.clone()
    calls = {
        "add_": lambda: parameter.add_(1),
        "view_copy_": lambda: parameter.t().copy_(torch.zeros(3, 2)),
        "out": lambda: torch.add(parameter, 1, out=parameter),
        "set_": lambda: parameter.set_(torch.zeros(2, 3)),
        "resize_": lambda: parameter.resize_(6),
    }
    errors = {}
    for name, call in calls.items():
        try:
            call()
        except RuntimeError as error:
            errors[name] = str(error)
        else:
            raise AssertionError(f"mutation was accepted: {name}")
    assert torch.equal(parameter.elem, before)
    return errors


def copies():
    _, parameter = installed()
    clone = parameter.clone()
    cast = parameter.to(torch.float64)
    assert type(clone) is torch.Tensor and type(cast) is torch.Tensor
    clone.add_(100)
    cast.add_(200)
    assert torch.equal(parameter.elem, torch.arange(6.0).reshape(2, 3))
    target = torch.empty(2, 3)
    result = torch.add(parameter, 1, out=target)
    assert result is target and torch.equal(target, parameter.elem + 1)
    return {"clone_mutable": True, "cast_mutable": True, "ordinary_out_target_supported": True}


def module_cast():
    module, parameter = installed()
    before = id(parameter)
    with conversion():
        module.to(dtype=torch.float64)
    assert module.left.weight is module.right.weight is parameter
    assert id(parameter) == before and parameter.dtype == torch.float64
    assert isinstance(parameter, Weight)
    assert parameter.family == "cast:torch.float64"
    return {"identity": True, "ties": True, "dtype": str(parameter.dtype), "family": parameter.family}


def module_cast_with_view():
    module, parameter = installed()
    view = parameter.t()
    old = view.elem.clone()
    use_count = parameter._use_count()
    with conversion():
        module.to(dtype=torch.float64)
    assert parameter.dtype == torch.float64 and view.dtype == torch.float32
    assert torch.equal(view.elem, old)
    return {"old_view_retained": True, "before_use_count": use_count}


def swap_weakref():
    parameter = torch.nn.Parameter(torch.ones(2), requires_grad=False)
    reference = weakref.ref(parameter)
    replacement = torch.nn.Parameter(Weight(torch.ones(2)), requires_grad=False)
    try:
        torch.utils.swap_tensors(parameter, replacement)
    except RuntimeError as error:
        assert reference() is parameter and type(parameter) is torch.nn.Parameter
        return {"refused": str(error), "original_unchanged": True}
    raise AssertionError("weakref swap accepted")


def data_setter_bypass():
    _, parameter = installed()
    before_ops = len(OPS)
    parameter.data = torch.full((2, 3), 99.0)
    return {"setter_accepted": True, "dispatch_calls": OPS[before_ops:], "payload": parameter.elem.tolist(), "reported_data": (parameter + 0).tolist()}


def gradient_setter_bypass():
    _, parameter = installed()
    before_ops = len(OPS)
    parameter.requires_grad_(True)
    return {"setter_accepted": parameter.requires_grad, "dispatch_calls": OPS[before_ops:]}


def controlled_set_data_with_view():
    module, parameter = installed()
    view = parameter.t()
    before = id(parameter)
    replacement = Weight(parameter.elem.to(torch.float64), family="controlled-f64")
    # Experimental controlled rebind: the dispatcher-only probe exposes the exact
    # metadata/data setter mechanics; not a proposed production bypass.
    parameter.data = replacement
    parameter.elem, parameter.family = replacement.elem, replacement.family
    assert id(parameter) == before and module.right.weight is parameter
    assert parameter.dtype == torch.float64 and view.dtype == torch.float32
    assert torch.equal(view + 0, torch.arange(6.0).reshape(2, 3).t())
    assert torch.equal(parameter + 0, torch.arange(6.0, dtype=torch.float64).reshape(2, 3))
    return {"identity": True, "ties": True, "old_view_retained": True, "new_dtype": str(parameter.dtype), "old_dtype": str(view.dtype)}


for name, fn in [
    ("identity_and_real_forward", identity_forward), ("view_detach_after_module_drop", aliases),
    ("schema_write_rejection", mutations), ("ordinary_copy_results", copies),
    ("controlled_module_apply", module_cast), ("module_apply_with_escaped_view", module_cast_with_view),
    ("swap_weakref_refusal", swap_weakref), ("data_setter_dispatch_bypass", data_setter_bypass),
    ("requires_grad_dispatch_bypass", gradient_setter_bypass),
    ("controlled_set_data_with_old_view", controlled_set_data_with_view),
]:
    case(name, fn)

assert not torch.cuda.is_initialized()
assert not any("libcuda.so" in line for line in Path("/proc/self/maps").read_text().splitlines())
Path(__file__).with_suffix(".json").write_text(json.dumps({"torch": torch.__version__, "cuda": False, "threads": torch.get_num_threads(), "scope": "real CPU Torch semantics only; no Varena residency proof", "results": RESULTS}, indent=2) + "\n")
for result in RESULTS:
    print(result["name"], result["outcome"], result.get("error", result.get("detail")))
