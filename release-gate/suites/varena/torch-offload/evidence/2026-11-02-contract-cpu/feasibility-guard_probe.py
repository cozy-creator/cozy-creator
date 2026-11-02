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
                    return type(weight)(value, family=weight.family)
            if CONVERT.get() and func is torch.ops.aten._to_copy.default:
                return cls(value, family=f"cast:{value.dtype}")
            return value

        return return_and_correct_aliasing(func, args, kwargs, tree_map(wrap, result))



ALLOW_REBIND = contextvars.ContextVar("allow_controlled_rebind", default=False)
FUNCTIONS = []
class GuardedWeight(Weight):
    @classmethod
    def __torch_function__(cls, func, types, args=(), kwargs=None):
        kwargs = kwargs or {}
        name = getattr(func, "__name__", "")
        descriptor = getattr(func, "__self__", None)
        FUNCTIONS.append({"name": name, "descriptor": str(descriptor)})
        if name == "__set__" and descriptor is torch.Tensor.data and not ALLOW_REBIND.get():
            raise RuntimeError("managed .data assignment rejected")
        if name == "__set__" and descriptor is torch.Tensor.requires_grad and bool(args[1]):
            raise RuntimeError("managed requires_grad assignment rejected")
        if func is torch.Tensor.requires_grad_ and bool(args[1] if len(args)>1 else kwargs.get("requires_grad", True)):
            raise RuntimeError("managed requires_grad_ rejected")
        with torch._C.DisableTorchFunctionSubclass():
            return func(*args, **kwargs)

RESULTS=[]
def case(name, fn):
    start=len(FUNCTIONS)
    try:
        detail=fn()
        RESULTS.append({"name":name,"outcome":"pass","detail":detail,"functions":FUNCTIONS[start:]})
    except Exception as e:
        RESULTS.append({"name":name,"outcome":"observed_failure","error":str(e),"traceback":traceback.format_exc(),"functions":FUNCTIONS[start:]})

def make():
    original=torch.nn.Parameter(torch.arange(6.0).reshape(2,3),requires_grad=False)
    candidate=torch.nn.Parameter(GuardedWeight(original.detach().clone()),requires_grad=False)
    torch.utils.swap_tensors(original,candidate)
    return original

def setters():
    p=make()
    results={}
    for name,fn in {"data":lambda:setattr(p,"data",torch.zeros(2,3)),"requires_grad_attr":lambda:setattr(p,"requires_grad",True),"requires_grad_method":lambda:p.requires_grad_(True)}.items():
        try: fn()
        except RuntimeError as e: results[name]=str(e)
        else: raise AssertionError(name+" was accepted")
    assert torch.equal(p+0,torch.arange(6.0).reshape(2,3))
    return results

def aliases_and_copy():
    p=make()
    d=p.data
    assert isinstance(d,GuardedWeight)
    try:d.add_(1)
    except RuntimeError:pass
    else:raise AssertionError("data alias mutated")
    clone=p.clone()
    assert type(clone) is torch.Tensor
    clone.add_(1)
    return {"data_alias_guarded":True,"clone_plain_mutable":True,"storage_pointer":p.data_ptr()}

def manual_rebind():
    p=make(); old=p.t(); pid=id(p)
    new=GuardedWeight(p.elem.to(torch.float64),family="new-f64")
    token=ALLOW_REBIND.set(True)
    try:
        p.data=new
        p.elem,p.family=new.elem,new.family
    finally:ALLOW_REBIND.reset(token)
    assert id(p)==pid and p.dtype==torch.float64 and old.dtype==torch.float32
    assert torch.equal(old+0,torch.arange(6.0).reshape(2,3).t())
    return {"new_dtype":str(p.dtype),"old_dtype":str(old.dtype),"identity":True,"old_view_values":(old+0).tolist()}

def hidden_batch_norm_write():
    mean=GuardedWeight(torch.zeros(3)); variance=GuardedWeight(torch.ones(3))
    old_mean=mean.elem.clone(); old_var=variance.elem.clone()
    output=torch.ops.aten.native_batch_norm.default(torch.arange(12.0).reshape(4,3),None,None,mean,variance,True,0.5,1e-5)
    changed=not torch.equal(mean.elem,old_mean) or not torch.equal(variance.elem,old_var)
    assert changed, "this case is expected to expose the unannotated mutation"
    return {"schema":str(torch.ops.aten.native_batch_norm.default._schema),"mutation_bypassed_write_alias_guard":changed,"mean":mean.elem.tolist(),"variance":variance.elem.tolist(),"output_shape":list(output[0].shape)}

for name,fn in [("torch_function_setter_guards",setters),("data_alias_and_copy",aliases_and_copy),("controlled_rebind_with_guards",manual_rebind),("schema_only_batchnorm_counterexample",hidden_batch_norm_write)]:case(name,fn)
assert not torch.cuda.is_initialized()
assert not any("libcuda.so" in line for line in Path("/proc/self/maps").read_text().splitlines())
Path(__file__).with_suffix(".json").write_text(json.dumps({"torch":torch.__version__,"cuda":False,"threads":torch.get_num_threads(),"results":RESULTS},indent=2)+"\n")
for r in RESULTS:print(r["name"],r["outcome"],r.get("error",r.get("detail")))
