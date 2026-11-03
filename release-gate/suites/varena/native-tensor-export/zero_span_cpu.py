"""Real Torch CPU consumer-ABI evidence for zero-span DLPack storage pointers.

This client supplies explicit addresses in actual CPU DLPack capsules backed by
ctypes allocations. It does not import TensorFS, call the production CUDA export
bridge, model a native Pin, or qualify GPU ownership/order/reclamation. The
logical-end control and allocation-base case are supplied independently; this
client does not reproduce the production pointer-normalization algorithm.
"""

from __future__ import annotations

import ctypes as C
import gc
import json
import struct
from dataclasses import dataclass

import torch


class Device(C.Structure):
    _fields_ = [("kind", C.c_int32), ("ordinal", C.c_int32)]


class Dtype(C.Structure):
    _fields_ = [("code", C.c_uint8), ("bits", C.c_uint8), ("lanes", C.c_uint16)]


class Tensor(C.Structure):
    _fields_ = [
        ("data", C.c_void_p),
        ("device", Device),
        ("ndim", C.c_int32),
        ("dtype", Dtype),
        ("shape", C.POINTER(C.c_int64)),
        ("strides", C.POINTER(C.c_int64)),
        ("byte_offset", C.c_uint64),
    ]


class Managed(C.Structure):
    pass


Delete = C.CFUNCTYPE(None, C.POINTER(Managed))
Managed._fields_ = [
    ("tensor", Tensor),
    ("context", C.c_void_p),
    ("deleter", Delete),
]


@dataclass
class Record:
    name: str
    base: int
    supplied_pointer: int
    releases: int = 0


_owners = {}
_records = {}
_errors = []
_next = 0


@Delete
def delete(pointer):
    # The real Torch Storage finalizer calls this actual DLPack manager callback.
    # A separate record survives release; the allocation and metadata do not.
    try:
        identity = pointer.contents.context
        record = _records[identity]
        record.releases += 1
        owner = _owners.pop(identity, None)
        if owner is None:
            _errors.append(f"duplicate release for {record.name}")
    except BaseException as error:  # noqa: BLE001 - report every foreign-deleter callback failure
        _errors.append(repr(error))


_capsule_valid = C.pythonapi.PyCapsule_IsValid
_capsule_valid.argtypes = [C.c_void_p, C.c_char_p]
_capsule_valid.restype = C.c_int
_capsule_pointer = C.pythonapi.PyCapsule_GetPointer
_capsule_pointer.argtypes = [C.c_void_p, C.c_char_p]
_capsule_pointer.restype = C.c_void_p
CapsuleDelete = C.CFUNCTYPE(None, C.c_void_p)


@CapsuleDelete
def capsule_delete(capsule):
    # Consumed capsules are renamed used_dltensor and no longer own the manager.
    if _capsule_valid(capsule, b"dltensor"):
        delete(C.cast(_capsule_pointer(capsule, b"dltensor"), C.POINTER(Managed)))


_capsule_new = C.pythonapi.PyCapsule_New
_capsule_new.argtypes = [C.c_void_p, C.c_char_p, CapsuleDelete]
_capsule_new.restype = C.py_object


def consume(name, *, code, bits, shape, strides, pointer_offset, payload=b""):
    """Consume one actual CPU capsule with an explicitly selected input pointer."""
    global _next
    _next += 1
    identity = _next
    allocation = (C.c_uint64 * 32)()  # 256 real host bytes, aligned for all cases.
    base = C.addressof(allocation)
    if payload:
        C.memmove(base, payload, len(payload))
    dimensions = (C.c_int64 * len(shape))(*shape)
    element_strides = (C.c_int64 * len(strides))(*strides)
    record = Record(name, base, base + pointer_offset)
    managed = Managed(
        Tensor(
            record.supplied_pointer,
            Device(1, 0),  # kDLCPU
            len(shape),
            Dtype(code, bits, 1),
            dimensions if shape else C.POINTER(C.c_int64)(),
            element_strides if strides else C.POINTER(C.c_int64)(),
            0,
        ),
        identity,
        delete,
    )
    _owners[identity] = (allocation, dimensions, element_strides, managed)
    _records[identity] = record
    capsule = _capsule_new(C.addressof(managed), b"dltensor", capsule_delete)
    tensor = torch.utils.dlpack.from_dlpack(capsule)
    del capsule
    gc.collect()
    assert record.releases == 0
    assert not _errors, _errors
    return tensor, record


def release_aliases(tensor, record):
    """Return a final detached/view alias and test all preceding aliases' drops."""
    detached = tensor.detach()
    alias = detached.as_strided(detached.shape, detached.stride())
    assert alias.untyped_storage().data_ptr() == record.supplied_pointer
    del detached
    gc.collect()
    assert record.releases == 0
    return alias


def empty_case(name, code, bits, shape, strides, pointer_offset, dtype):
    tensor, record = consume(
        name,
        code=code,
        bits=bits,
        shape=shape,
        strides=strides,
        pointer_offset=pointer_offset,
    )
    assert tensor.dtype == dtype and tensor.numel() == 0
    assert tuple(tensor.shape) == shape and tuple(tensor.stride()) == strides
    assert tensor.data_ptr() == 0
    assert tensor.untyped_storage().data_ptr() == record.supplied_pointer
    assert tensor.untyped_storage().nbytes() == 0
    alias = release_aliases(tensor, record)
    del tensor
    gc.collect()
    assert record.releases == 0
    del alias
    gc.collect()
    assert record.releases == 1
    return {
        "name": name,
        "dtype": str(dtype),
        "shape": shape,
        "strides": strides,
        "supplied_pointer_offset": pointer_offset,
        "tensor_data_ptr": 0,
        "storage_pointer_offset": record.supplied_pointer - record.base,
        "deleter_calls_after_final_alias": record.releases,
    }


def nonempty_case(
    name, *, code, bits, dtype, shape, strides, pointer_offset, payload, expected
):
    tensor, record = consume(
        name,
        code=code,
        bits=bits,
        shape=shape,
        strides=strides,
        pointer_offset=pointer_offset,
        payload=payload,
    )
    assert tensor.dtype == dtype
    assert tuple(tensor.shape) == shape and tuple(tensor.stride()) == strides
    assert tensor.data_ptr() == record.supplied_pointer
    assert tensor.untyped_storage().data_ptr() == record.supplied_pointer
    assert torch.equal(tensor, expected)
    alias = release_aliases(tensor, record)
    del tensor
    gc.collect()
    assert record.releases == 0
    assert torch.equal(alias, expected)
    del alias
    gc.collect()
    assert record.releases == 1
    return {
        "name": name,
        "dtype": str(dtype),
        "shape": shape,
        "strides": strides,
        "supplied_pointer_offset": pointer_offset,
        "exact_values": True,
        "deleter_calls_after_final_alias": record.releases,
    }


def main():
    assert not torch.cuda.is_initialized()
    cases = []
    # Control: Torch's zero tensor data_ptr() does not erase a supplied logical-end
    # DataPtr from its Storage. The valid-base case is separately supplied below.
    for label, code, bits, dtype in (
        ("uint8", 1, 8, torch.uint8),
        ("float32", 2, 32, torch.float32),
        ("int64", 0, 64, torch.int64),
    ):
        for pointer_offset, address in (
            (256, "logical_end_control"),
            (0, "valid_base"),
        ):
            cases.append(
                empty_case(
                    f"{label}_{address}", code, bits, (0,), (1,), pointer_offset, dtype
                )
            )
    cases.append(
        empty_case(
            "float32_multidim_valid_base",
            2,
            32,
            (2, 0, 3),
            (3, 3, 1),
            0,
            torch.float32,
        )
    )
    cases.append(
        nonempty_case(
            "uint8_interior_stride",
            code=1,
            bits=8,
            dtype=torch.uint8,
            shape=(3,),
            strides=(2,),
            pointer_offset=3,
            payload=bytes(range(64)),
            expected=torch.tensor([3, 5, 7], dtype=torch.uint8),
        )
    )
    cases.append(
        nonempty_case(
            "float32_interior_transpose",
            code=2,
            bits=32,
            dtype=torch.float32,
            shape=(3, 2),
            strides=(1, 3),
            pointer_offset=4,
            payload=struct.pack("=16f", *(index / 2 for index in range(16))),
            expected=torch.tensor([[0.5, 2.0], [1.0, 2.5], [1.5, 3.0]]),
        )
    )
    cases.append(
        nonempty_case(
            "int64_interior_scalar",
            code=0,
            bits=64,
            dtype=torch.int64,
            shape=(),
            strides=(),
            pointer_offset=8,
            payload=struct.pack("=4q", -7, 123456789, 9, 11),
            expected=torch.tensor(123456789, dtype=torch.int64),
        )
    )
    gc.collect()
    assert not _owners and not _errors, (_owners, _errors)
    assert all(record.releases == 1 for record in _records.values())
    assert not torch.cuda.is_initialized()
    print(
        json.dumps(
            {
                "passed": True,
                "scope": "real Torch CPU DLPack consumer ABI; no production CUDA bridge or native Pin qualification",
                "torch": str(torch.__version__),
                "cuda_initialized": False,
                "allocation_bytes_per_case": 256,
                "cases": cases,
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
