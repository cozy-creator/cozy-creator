"""Real resident NVFP4 decode through the installed SDK and ordinary Cozy GPU lane."""

import hashlib
import itertools
import json
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Request(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 103


class Report(msgspec.Struct):
    passed: bool
    cases: int
    refusals: int
    detail: str


def specimen(torch, shape, layout, order, global_mode, with_pre, seed):
    """Serialize explicit FP4 codes/scales; expected values use the declared value law."""
    count, cols, height = int(torch.tensor(shape).prod()), shape[-1], shape[-2]
    rows, blocks = count // cols, cols // 16
    codes = (
        (torch.arange(count, dtype=torch.int64) + seed)
        .remainder(16)
        .reshape(rows, cols)
    )
    low, high = codes[:, 0::2], codes[:, 1::2]
    packed = ((low | high << 4) if order == "low_first" else (low << 4 | high)).to(
        torch.uint8
    )
    scales = 2.0 ** (torch.arange(rows * blocks).remainder(4).reshape(rows, blocks) - 2)
    scale_bytes = scales.to(torch.float8_e4m3fn).view(torch.uint8)
    matrices = rows // height
    if layout == "row_major":
        scale_shape = (*shape[:-1], blocks)
        stored_scales = scale_bytes.reshape(scale_shape)
    else:
        padded_rows, padded_blocks = (height + 127) // 128 * 128, (blocks + 3) // 4 * 4
        scale_shape = (*shape[:-2], padded_rows, padded_blocks)
        stored_scales = torch.zeros(
            matrices * padded_rows * padded_blocks, dtype=torch.uint8
        )
        # The format's documented cuBLAS tile serialization; padding is not logical data.
        for row in range(rows):
            matrix, local = divmod(row, height)
            for block in range(blocks):
                tile = (local // 128) * (padded_blocks // 4) + block // 4
                at = matrix * padded_rows * padded_blocks + tile * 512
                at += (local % 32) * 16 + (local % 128 // 32) * 4 + block % 4
                stored_scales[at] = scale_bytes[row, block]
        stored_scales = stored_scales.reshape(scale_shape)
    global_scale = torch.tensor([2.0 if i % 2 == 0 else 0.5 for i in range(matrices)])
    parts = {
        "weight": ("F4", shape, packed.reshape(*shape[:-1], cols // 2)),
        "scale": ("F8_E4M3", scale_shape, stored_scales),
        "scale_2": ("F32", (matrices,), global_scale),
        # Activation metadata must not become another factor in reconstructed weights.
        "input_scale": ("F32", (matrices,), torch.full((matrices,), 7.0)),
    }
    table = torch.tensor(
        [0, 0.5, 1, 1.5, 2, 3, 4, 6, -0.0, -0.5, -1, -1.5, -2, -3, -4, -6]
    )
    expected = table[codes] * scales.repeat_interleave(16, dim=1)
    factor = global_scale.repeat_interleave(height).reshape(rows, 1)
    expected = expected / factor if global_mode == "divide" else expected * factor
    if with_pre:
        pre = (2.0 ** (torch.arange(cols).remainder(3) - 1)).to(torch.bfloat16)
        parts["pre_quant_scale"] = ("BF16", (cols,), pre)
        expected *= pre.float()
    params = {
        "block_size": 16,
        "global_scale": global_mode,
        "nibble_order": order,
        "scale_layout": layout,
    }
    return parts, params, expected.reshape(shape)


@app.entrypoint(accelerator=True)
def run(payload: Request, ctx: Context) -> Report:
    import tensorfs
    import torch
    from tensorfs import raw

    if ctx.device.type != "cuda":
        raise RuntimeError("resident decoding requires a real CUDA device")
    torch.set_num_threads(1)
    rows = []
    producer = torch.cuda.Stream()
    shapes = [(3, 32), (2, 129, 80)]
    variants = itertools.product(
        shapes,
        ("row_major", "cublas_128x4"),
        ("low_first", "high_first"),
        ("multiply", "divide"),
        (False, True),
        (("F32", torch.float32), ("BF16", torch.bfloat16), ("F16", torch.float16)),
    )
    for shape, layout, order, global_mode, with_pre, (dtype, torch_dtype) in variants:
        host, params, expected = specimen(
            torch, shape, layout, order, global_mode, with_pre, payload.seed
        )
        reference = torch.empty(shape, dtype=torch_dtype)
        raw.decode_into(
            "nvfp4", host, reference, dtype=dtype, shape=shape, params=params
        )
        torch.testing.assert_close(reference, expected.to(torch_dtype), rtol=0, atol=0)
        with torch.cuda.stream(producer):
            parts = {
                name: (kind, dimensions, data.to("cuda", non_blocking=True))
                for name, (kind, dimensions, data) in host.items()
            }
            out = torch.empty(shape, dtype=torch_dtype, device="cuda")
            raw.decode_into(
                "nvfp4", parts, out, dtype=dtype, shape=shape, params=params
            )
        # decode_into success is the producer completion certificate, not a later device join.
        actual = out.cpu()
        torch.testing.assert_close(actual, reference, rtol=0, atol=0)
        rows.append(
            {
                "shape": shape,
                "layout": layout,
                "order": order,
                "global": global_mode,
                "pre": with_pre,
                "dtype": dtype,
                "sha256": hashlib.sha256(
                    actual.view(torch.uint8).numpy().tobytes()
                ).hexdigest(),
            }
        )

    # Real malformed metadata, overlap and mixed-placement requests must refuse before writes.
    shape = shapes[0]
    host, params, _ = specimen(
        torch, shape, "row_major", "low_first", "multiply", False, payload.seed
    )
    parts = {
        name: (kind, dimensions, data.to("cuda"))
        for name, (kind, dimensions, data) in host.items()
    }
    out = torch.full(shape, 123.0, device="cuda")
    refusals = 0
    for supplied, options, expected_error, phrase in [
        (parts, {**params, "block_size": 32}, tensorfs.DecodeError, "block_size"),
        (
            {**parts, "scale_2": host["scale_2"]},
            params,
            tensorfs.UnsupportedError,
            "same device",
        ),
    ]:
        try:
            raw.decode_into(
                "nvfp4", supplied, out, dtype="F32", shape=shape, params=options
            )
        except (tensorfs.DecodeError, tensorfs.UnsupportedError) as error:
            assert isinstance(error, expected_error), type(error)
            assert phrase in str(error), str(error)
        else:
            raise AssertionError("invalid resident descriptor was accepted")
        assert bool((out == 123).all().item())
        refusals += 1
    carrier = torch.zeros(out.numel() * 4, dtype=torch.uint8, device="cuda")
    shared = carrier.view(torch.float32).reshape(shape)
    aliased = {**parts, "weight": ("F4", shape, carrier[: out.numel() // 2])}
    try:
        raw.decode_into(
            "nvfp4", aliased, shared, dtype="F32", shape=shape, params=params
        )
    except tensorfs.InvalidError as error:
        assert "overlap" in str(error), str(error)
    else:
        raise AssertionError("overlapping input/output was accepted")
    assert not bool(carrier.any().item())
    refusals += 1
    raw.decode_into("nvfp4", parts, out, dtype="F32", shape=shape, params=params)
    _, _, expected = specimen(
        torch, shape, "row_major", "low_first", "multiply", False, payload.seed
    )
    torch.testing.assert_close(out.cpu(), expected, rtol=0, atol=0)
    detail = {
        "cases": rows,
        "tensorfs": tensorfs.__version__,
        "native_sha256": hashlib.sha256(
            Path(tensorfs._ext.__file__).read_bytes()
        ).hexdigest(),
        "gpu": torch.cuda.get_device_name(),
        "producer_stream": "nondefault",
        "healthy_after_refusals": True,
    }
    return Report(True, len(rows), refusals, json.dumps(detail, sort_keys=True))
