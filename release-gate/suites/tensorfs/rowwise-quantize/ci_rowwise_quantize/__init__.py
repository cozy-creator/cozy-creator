"""Real bounded rowwise source decoding and native checkpoint quantization."""

import hashlib
import itertools
import json
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Request(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 103
    mode: str = "basic"


class Report(msgspec.Struct):
    passed: bool
    cases: int
    detail: str


@app.entrypoint(accelerator=False)
def run(payload: Request, ctx: Context) -> Report:
    import numpy as np
    import tensorfs
    import torch
    from cozy_runtime.derive.microscale import encode_int8_convrot256, encode_nvfp4
    from cozy_runtime.internal.encoding.formats import RowwiseDequant

    torch.set_num_threads(1)
    if payload.mode == "cancel":
        return cancel_rowwise(payload.seed, tensorfs, np)
    assert payload.mode == "basic"
    rows, cols = 7, 256
    kinds = [
        ("fp8.rowwise", "F8_E4M3", torch.float8_e4m3fn),
        ("fp8.rowwise", "F8_E5M2", torch.float8_e5m2),
        ("int8.rowwise", "I8", torch.int8),
    ]
    scale_kinds = [
        ("F32", torch.float32),
        ("F16", torch.float16),
        ("BF16", torch.bfloat16),
    ]
    logical = ["F32", "F16", "BF16"]
    cases = []
    with tempfile.TemporaryDirectory(prefix="cozy-rowwise-quantize-") as temporary:
        directory = Path(temporary)
        writer = tensorfs.Writer(directory / "source.cozytensors")
        component = writer.component("net", config={"seed": payload.seed})
        component.put("keep", np.arange(9, dtype=np.float32))
        for index, (
            (scheme, weight_dtype, element),
            (scale_dtype, scale_type),
            dtype,
        ) in enumerate(itertools.product(kinds, scale_kinds, logical)):
            codes = (
                ((torch.arange(rows * cols) + payload.seed).remainder(256))
                .to(torch.uint8)
                .reshape(rows, cols)
            )
            weight = codes.view(element)
            if scheme == "fp8.rowwise":
                codes[~torch.isfinite(weight.float())] = 0
                weight = codes.view(element)
            scales = torch.tensor(
                [0.0, -0.125, 0.333251953125, 1.00390625, 2.0, 0.0001220703125, 32.0],
                dtype=scale_type,
            )
            values = torch.empty(rows, cols, dtype=torch.float32)
            RowwiseDequant(scheme).decode(
                torch, {"weight": weight, "scale": scales}, values
            )
            decoded = torch.empty_like(values)
            tensorfs.raw.decode_into(
                scheme,
                {
                    "weight": (weight_dtype, (rows, cols), codes),
                    "scale": (scale_dtype, (rows,), scales),
                },
                decoded,
                dtype="F32",
                shape=(rows, cols),
                params={},
            )
            assert torch.equal(decoded.view(torch.int32), values.view(torch.int32))
            key = f"matrix{index}.weight"
            component.put_encoded(
                key,
                dtype=dtype,
                shape=(rows, cols),
                scheme=scheme,
                params={},
                parts={
                    "weight": (weight_dtype, (rows, cols), codes.numpy().tobytes()),
                    "scale": (
                        scale_dtype,
                        (rows,),
                        scales.view(torch.uint8).numpy().tobytes(),
                    ),
                },
            )
            cases.append((key, dtype, weight_dtype, scale_dtype, values.numpy().copy()))
        source = writer.commit()
        summaries = []
        for target, strategy, params, producer in [
            (
                "int8.convrot",
                "int8-convrot256/1",
                {"rotation": "kron_regular_h4", "rotation_size": 256},
                encode_int8_convrot256,
            ),
            (
                "nvfp4",
                "nvfp4-min-sse/1",
                {
                    "block_size": 16,
                    "global_scale": "multiply",
                    "nibble_order": "low_first",
                    "scale_layout": "row_major",
                },
                encode_nvfp4,
            ),
        ]:
            selected = [
                {
                    "component": "net",
                    "key": key,
                    "source_dtype": dtype,
                    "shape": [rows, cols],
                    "dtype": dtype,
                    "encoding": {"scheme": target, "params": params},
                    "strategy": strategy,
                }
                for key, dtype, _, _, _ in cases
            ]
            expected_read = sum(
                rows * cols + rows * (4 if scale_dtype == "F32" else 2)
                for _, _, _, scale_dtype, _ in cases
            )
            if target == "nvfp4":
                expected_read *= 2
            encoded_bytes = len(cases) * (
                rows * cols + rows * 4
                if target == "int8.convrot"
                else rows * cols // 2 + rows * cols // 16 + 4
            )
            # Worst source decode row: stored bytes + I32 codes + F32 values + scales.
            # 4KiB fixed scratch plus one F64 encoder row bounds all27 source variants.
            per_row = 9 * cols + 8
            fixed = 4096 + (8 * cols if target == "int8.convrot" else 0)
            previous = None
            for chunk in (1, 2, 7):
                output = tensorfs.Writer(
                    directory / f"{target}-{chunk}.cozytensors", base=source
                )
                control = tensorfs.ConversionControl()
                stats = output.quantize(
                    source,
                    selected,
                    max_buffer_bytes=fixed + per_row * chunk,
                    max_new_bytes=encoded_bytes,
                    control=control,
                )
                assert stats["source_bytes_read"] == expected_read
                assert control.progress()[0] == expected_read
                assert (
                    stats["encoded_keys"] == len(cases)
                    and stats["new_bytes_written"] == encoded_bytes
                )
                checkpoint = output.commit()
                assert checkpoint.verify().ok
                digests = {}
                measured = {row["key"]: row["measured"] for row in stats["tensors"]}
                for key, _, _, _, values in cases:
                    expected = producer(key, values)
                    actual = checkpoint.components["net"].tensors.read(key)
                    assert actual.parts["weight"].tobytes() == expected.payload, (
                        key,
                        target,
                        chunk,
                    )
                    for role, part in zip(("scale", "scale_2"), expected.companions):
                        assert actual.parts[role].tobytes() == part.raw, (
                            key,
                            role,
                            target,
                            chunk,
                        )
                    m = measured[key]
                    assert (
                        m["max_abs_error"] == expected.max_abs_err
                        and m["saturated"] == expected.saturated
                    )
                    np.testing.assert_allclose(
                        m["sse"], expected.sse, rtol=2e-13, atol=0
                    )
                    np.testing.assert_allclose(
                        m["energy"], expected.energy, rtol=2e-13, atol=0
                    )
                    digests[key] = hashlib.sha256(
                        expected.payload
                        + b"".join(part.raw for part in expected.companions)
                    ).hexdigest()
                if previous is not None:
                    assert previous == digests
                previous = digests
                assert (
                    checkpoint.components["net"].tensors.read("keep").tobytes()
                    == np.arange(9, dtype=np.float32).tobytes()
                )
                summaries.append(
                    {
                        "target": target,
                        "chunk_rows": chunk,
                        "read_bytes": expected_read,
                        "new_bytes": encoded_bytes,
                        "identity": stats["identity"],
                        "digests": digests,
                    }
                )
                checkpoint.close()
        refusals = []
        for name, options in [
            ("params", {"params": {"extra": 1}}),
            ("scale_shape", {"scale_shape": (rows, 1)}),
            ("weight_dtype", {"weight_dtype": "I8"}),
            ("scale_dtype", {"scale_dtype": "I32"}),
            ("extra_role", {"extra": True}),
            ("nonfinite", {"nonfinite": True}),
        ]:
            invalid_source = tensorfs.Writer(directory / f"bad-{name}.cozytensors")
            bad_parts = {
                "weight": (
                    options.get("weight_dtype", "F8_E4M3"),
                    (rows, cols),
                    torch.full((rows, cols), 0x38, dtype=torch.uint8),
                ),
                "scale": (
                    options.get("scale_dtype", "F32"),
                    options.get("scale_shape", (rows,)),
                    torch.full(
                        (rows,), float("nan") if options.get("nonfinite") else 1.0
                    ),
                ),
            }
            if options.get("extra"):
                bad_parts["extra"] = ("F32", (1,), torch.ones(1))
            bad_parts = {
                role: (dtype, shape, data.view(torch.uint8).numpy().tobytes())
                for role, (dtype, shape, data) in bad_parts.items()
            }
            invalid_source.component("net").put_encoded(
                "bad.weight",
                dtype="BF16",
                shape=(rows, cols),
                scheme="fp8.rowwise",
                params=options.get("params", {}),
                parts=bad_parts,
            )
            bad_checkpoint = invalid_source.commit()
            output = tensorfs.Writer(
                directory / f"bad-out-{name}.cozytensors", base=bad_checkpoint
            )
            selected_bad = [
                {
                    "component": "net",
                    "key": "bad.weight",
                    "source_dtype": "BF16",
                    "shape": [rows, cols],
                    "dtype": "BF16",
                    "encoding": {
                        "scheme": "int8.convrot",
                        "params": {"rotation": "kron_regular_h4", "rotation_size": 256},
                    },
                    "strategy": "int8-convrot256/1",
                }
            ]
            control = tensorfs.ConversionControl()
            try:
                output.quantize(
                    bad_checkpoint,
                    selected_bad,
                    max_buffer_bytes=16 << 10,
                    max_new_bytes=1 << 20,
                    control=control,
                )
            except (tensorfs.DecodeError, tensorfs.UnsupportedError) as error:
                assert not output.has("net", "bad.weight")
                if name != "nonfinite":
                    assert control.progress()[0] == 0, (name, control.progress())
                refusals.append(
                    {
                        "case": name,
                        "code": error.code,
                        "read_bytes": control.progress()[0],
                    }
                )
            else:
                raise AssertionError(f"invalid rowwise source {name} was accepted")
            output.abandon()
            bad_checkpoint.close()
        return Report(
            True,
            len(cases) * 2 * 3,
            json.dumps(
                {
                    "tensorfs": tensorfs.__version__,
                    "numpy": np.__version__,
                    "torch": torch.__version__,
                    "source_header": source.id,
                    "seed": payload.seed,
                    "native_sha256": hashlib.sha256(
                        Path(tensorfs._ext.__file__).read_bytes()
                    ).hexdigest(),
                    "source_cases": len(cases),
                    "refusals": refusals,
                    "summaries": summaries,
                },
                sort_keys=True,
            ),
        )


def cancel_rowwise(seed, tensorfs, np):
    with tempfile.TemporaryDirectory(prefix="cozy-rowwise-cancel-") as temporary:
        directory = Path(temporary)
        writer = tensorfs.Writer(directory / "source.cozytensors")
        component = writer.component("net")
        selected = []
        for key, shape in [("first", (1, 256)), ("large", (16384, 1024))]:
            codes = np.resize(np.arange(-128, 128, dtype=np.int8), shape)
            component.put_encoded(
                key,
                dtype="BF16",
                shape=shape,
                scheme="int8.rowwise",
                params={},
                parts={
                    "weight": ("I8", shape, codes),
                    "scale": ("F32", (shape[0],), np.ones(shape[0], dtype=np.float32)),
                },
            )
            selected.append(
                {
                    "component": "net",
                    "key": key,
                    "source_dtype": "BF16",
                    "shape": list(shape),
                    "dtype": "BF16",
                    "encoding": {
                        "scheme": "int8.convrot",
                        "params": {"rotation": "kron_regular_h4", "rotation_size": 256},
                    },
                    "strategy": "int8-convrot256/1",
                }
            )
        source = writer.commit()
        output = tensorfs.Writer(directory / "converted.cozytensors", base=source)
        control = tensorfs.ConversionControl()
        with ThreadPoolExecutor(max_workers=1) as pool:
            future = pool.submit(
                output.quantize,
                source,
                selected,
                max_buffer_bytes=128 << 10,
                max_new_bytes=32 << 20,
                control=control,
            )
            until = time.monotonic() + 20
            while control.progress()[1] != 1 or control.progress()[0] <= 260:
                assert not future.done(), "real rowwise cancellation window missed"
                assert time.monotonic() < until, control.progress()
                time.sleep(0.001)
            control.cancel()
            try:
                future.result(timeout=20)
            except tensorfs.NetworkError as error:
                assert "stopped" in str(error)
            else:
                raise AssertionError("rowwise conversion completed before cancellation")
        assert output.has("net", "first") and not output.has("net", "large")
        resumed = output.quantize(
            source,
            selected,
            max_buffer_bytes=256 << 10,
            max_new_bytes=32 << 20,
            control=tensorfs.ConversionControl(),
        )
        assert resumed["encoded_keys"] == 1 and resumed["reused_keys"] == 1
        assert resumed["source_bytes_read"] == 16384 * (1024 + 4)
        assert resumed["worst_relative_frobenius"] is None
        checkpoint = output.commit()
        assert checkpoint.verify().ok
        return Report(
            True,
            2,
            json.dumps(
                {
                    "mode": "cancel",
                    "seed": seed,
                    "cancel_progress": control.progress(),
                    "resumed": resumed,
                    "native_sha256": hashlib.sha256(
                        Path(tensorfs._ext.__file__).read_bytes()
                    ).hexdigest(),
                },
                sort_keys=True,
            ),
        )
