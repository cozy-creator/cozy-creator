"""Real bounded rowwise source decoding and native checkpoint quantization."""

import hashlib
import itertools
import json
import tempfile
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Request(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 103


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
                [0.0, 0.125, 0.333251953125, 1.00390625, 2.0, 0.0001220703125, 32.0],
                dtype=scale_type,
            )
            values = torch.empty(rows, cols, dtype=torch.float32)
            RowwiseDequant(scheme).decode(
                torch, {"weight": weight, "scale": scales}, values
            )
            key = f"matrix{index}.weight"
            component.put_encoded(
                key,
                dtype=dtype,
                shape=(rows, cols),
                scheme=scheme,
                params={},
                parts={
                    "weight": (weight_dtype, (rows, cols), codes),
                    "scale": (scale_dtype, (rows,), scales),
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
                    "summaries": summaries,
                },
                sort_keys=True,
            ),
        )
