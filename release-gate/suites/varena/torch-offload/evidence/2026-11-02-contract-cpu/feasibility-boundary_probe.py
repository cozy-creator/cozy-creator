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
        OPS.append({"func":str(func),"grad_enabled_in_dispatch":torch.is_grad_enabled()})
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



RESULTS=[]
def case(name,fn):
    try: RESULTS.append({"name":name,"outcome":"pass","detail":fn()})
    except Exception as e: RESULTS.append({"name":name,"outcome":"observed_failure","error":str(e),"traceback":traceback.format_exc()})

def grad_input():
    module,_=installed()
    value=torch.arange(3.0).reshape(1,3).requires_grad_()
    start=len(OPS)
    output=module(value)
    output.sum().backward()
    assert output.requires_grad and torch.equal(value.grad,torch.tensor([[6.,10.,14.]]))
    return {"eval_mode":not module.training,"managed_parameter_requires_grad":module.left.weight.requires_grad,"output_requires_grad":output.requires_grad,"input_gradient":value.grad.tolist(),"ops":OPS[start:]}

def readonly_dlpack():
    from torch.utils.dlpack import ReadOnlyTensorWrapper
    source=torch.arange(4.0)
    wrapped=ReadOnlyTensorWrapper(source)
    imported=torch.from_dlpack(wrapped)
    imported.add_(10)
    assert torch.equal(source,torch.arange(4.0)+10)
    return {"read_only_export_did_not_prevent_imported_mutation":True,"source_after":source.tolist(),"same_storage":source.untyped_storage().data_ptr()==imported.untyped_storage().data_ptr()}

def parameter_metadata():
    original=torch.nn.Parameter(torch.ones(2),requires_grad=False)
    original.user_tag="retained-before"
    identity=id(original)
    replacement=torch.nn.Parameter(Weight(torch.ones(2)),requires_grad=False)
    torch.utils.swap_tensors(original,replacement)
    assert id(original)==identity
    return {"identity_preserved":True,"custom_attribute_preserved":getattr(original,"user_tag",None)=="retained-before","original_attribute_moved_to_replacement":getattr(replacement,"user_tag",None)}

def preexisting_plain_view():
    original=torch.nn.Parameter(torch.arange(6.0).reshape(2,3),requires_grad=False)
    view=original.t()
    replacement=torch.nn.Parameter(Weight(original.detach().clone()),requires_grad=False)
    before=original._use_count()
    try:torch.utils.swap_tensors(original,replacement)
    except RuntimeError as e:return {"use_count":before,"refused":str(e),"old_view_still_valid":torch.equal(view,torch.arange(6.0).reshape(2,3).t())}
    raise AssertionError("expected preexisting-view swap refusal")

for name,fn in [("eval_and_frozen_parameters_do_not_disable_autograd",grad_input),("read_only_dlpack_import_is_mutable",readonly_dlpack),("parameter_dict_moves_during_swap",parameter_metadata),("preexisting_plain_view_refusal",preexisting_plain_view)]:case(name,fn)
assert not torch.cuda.is_initialized()
assert not any("libcuda.so" in line for line in Path("/proc/self/maps").read_text().splitlines())
Path(__file__).with_suffix(".json").write_text(json.dumps({"torch":torch.__version__,"cuda":False,"threads":torch.get_num_threads(),"future_swap_flag":torch.__future__.get_swap_module_params_on_conversion(),"results":RESULTS},indent=2)+"\n")
for r in RESULTS:print(r["name"],r["outcome"],r.get("error",r.get("detail")))
