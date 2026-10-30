"""The tensorfs suite's ComfyUI H3 layout cell: the sha256 of block 0's and the first refiner block's
attention and fc1 projections in a model's DiT, as TensorFS imported it. check.py holds them against
the official MiniMax-H3 Ref2VA bytes: an import that read ComfyUI's contiguous [q; k; v] the official
interleaved way (renders grey: runs 20078, 20085) changes every one."""

import hashlib
from typing import Any

import msgspec
from cozy_runtime.author import App, Context, Weights, invocable

app = App()
KEYS = tuple(
    f"{block}.{module}.weight"
    for block in ("transformer_blocks.0", "token_refiner.refiner_blocks.0")
    for module in ("attn.to_q", "attn.to_k", "attn.to_v", "ff.net.0.proj")
)
WIDTH = {"bf16": 2, "f16": 2, "f32": 4}


class Hash(msgspec.Struct):
    key: str
    sha256: str


class Hashes(msgspec.Struct):
    component: str
    hashes: list[Hash]


@invocable
async def rows(ctx: Context, *, dit: Weights[Any]) -> Hashes:
    source = ctx.tensorfs_source(dit)
    name, component = next((n, c) for n, c in source.components.items() if KEYS[0] in c.tensors)
    out = []
    for key in KEYS:
        row = component.tensors[key]
        size = WIDTH[row.dtype.lower()]
        for dim in row.shape:
            size *= dim
        buffer = bytearray(size)
        component.tensors.read_into(key, memoryview(buffer))
        out.append(Hash(key, hashlib.sha256(buffer).hexdigest()))
    return Hashes(name, out)


app.entrypoint(rows, name="rows", accelerator=False)
