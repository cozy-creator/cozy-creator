"""Native min-SSE bytes versus the actual Runtime producer, through the ordinary Cozy lane."""

import hashlib
import inspect
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
    refusals: int
    detail: str


@app.entrypoint(accelerator=False)
def run(payload: Request, ctx: Context) -> Report:
    import numpy as np
    import tensorfs
    from cozy_runtime.derive.microscale import encode_nvfp4
    from tensorfs import raw

    params = {
        "block_size": 16,
        "global_scale": "multiply",
        "nibble_order": "low_first",
        "scale_layout": "row_major",
    }

    def encode(values, maximum, dtype="F32", **overrides):
        options = {
            "dtype": dtype,
            "shape": values.shape,
            "strategy": "nvfp4-min-sse/1",
            "params": params,
            "tensor_amax": maximum,
        }
        options.update(overrides)
        return raw.quantize("nvfp4", values.astype("<f4").tobytes(), **options)

    def stored(encoded):
        return {
            role: (part.dtype, part.shape, part.tobytes())
            for role, part in encoded.parts.items()
        }

    rng = np.random.default_rng(payload.seed)
    outliers = rng.uniform(-1, 1, size=(17, 64)).astype(np.float32)
    outliers[-1, -1] = 1e6
    small = np.full(
        (3, 32), np.nextafter(np.float32(0), np.float32(1)), dtype=np.float32
    )
    small[-1, -1] = 1.0
    cases = {
        "zeros": np.zeros((3, 32), dtype=np.float32),
        "negative_zero": np.full((2, 16), -0.0, dtype=np.float32),
        "ties": np.tile(
            np.array(
                [
                    0.0,
                    -0.0,
                    0.25,
                    -0.25,
                    0.75,
                    -0.75,
                    1.25,
                    -1.25,
                    2.5,
                    -2.5,
                    3.5,
                    -3.5,
                    5,
                    -5,
                    6,
                    -6,
                ],
                dtype=np.float32,
            ),
            (5, 4),
        ),
        "last_chunk_maximum": outliers,
        "subnormals_with_shared_maximum": small,
        "tiny_normal": rng.uniform(-1, 1, size=(5, 32)).astype(np.float32)
        * np.float32(1e-35),
        "mixed_exponents": (
            rng.normal(size=(17, 64)) * np.exp2(rng.integers(-90, 90, size=(17, 64)))
        ).astype(np.float32),
        "maximum_f32": np.tile(
            np.array(
                [np.finfo(np.float32).max, -np.finfo(np.float32).max, 0, -0.0],
                dtype=np.float32,
            ),
            (3, 8),
        ),
    }
    records = []
    with tempfile.TemporaryDirectory(prefix="cozy-nvfp4-encode-") as directory:
        with tensorfs.Writer(Path(directory) / "encoded.cozytensors") as writer:
            writer.metadata["quantizer"] = "nvfp4-min-sse/1"
            component = writer.component("weights")
            references = {}
            for name, values in cases.items():
                maximum = float(np.abs(values).max())
                with np.errstate(all="ignore"):
                    reference = encode_nvfp4(name, values, amax=maximum)
                for dtype in ("F32", "F16", "BF16"):
                    result = encode(values, maximum, dtype)
                    parts = stored(result)
                    assert result.dtype == dtype and result.shape == values.shape
                    assert result.scheme == "nvfp4" and result.params == params
                    assert parts["weight"] == ("F4", values.shape, reference.payload), (
                        name
                    )
                    assert parts["scale"] == (
                        "F8_E4M3",
                        (values.shape[0], values.shape[1] // 16),
                        reference.companions[0].raw,
                    ), name
                    assert parts["scale_2"] == (
                        "F32",
                        (),
                        reference.companions[1].raw,
                    ), name
                    key = f"{name}.{dtype}"
                    component.put_encoded(
                        key,
                        dtype=dtype,
                        shape=values.shape,
                        scheme=result.scheme,
                        params=result.params,
                        parts=parts,
                    )
                    references[key] = parts
                for chunk_rows in (1, 2, 7):
                    chunks = [
                        stored(encode(values[start : start + chunk_rows], maximum))
                        for start in range(0, len(values), chunk_rows)
                    ]
                    assert (
                        b"".join(c["weight"][2] for c in chunks) == reference.payload
                    ), name
                    assert (
                        b"".join(c["scale"][2] for c in chunks)
                        == reference.companions[0].raw
                    ), name
                    assert all(
                        c["scale_2"][2] == reference.companions[1].raw for c in chunks
                    ), name
                records.append(
                    {
                        "name": name,
                        "shape": values.shape,
                        "tensor_amax": maximum,
                        "part_sha256": {
                            role: hashlib.sha256(part[2]).hexdigest()
                            for role, part in parts.items()
                        },
                    }
                )
            checkpoint = writer.commit()
        for key, expected in references.items():
            result = checkpoint.components["weights"].tensors.read(key)
            assert stored(result) == expected, key
        for name, values in cases.items():
            result = checkpoint.components["weights"].tensors.read(f"{name}.F32")
            out = np.empty(values.shape, dtype=np.float32)
            raw.decode_into(
                result.scheme,
                stored(result),
                out,
                dtype="F32",
                shape=values.shape,
                params=result.params,
            )
            loaded = checkpoint.components["weights"].tensors.read(
                f"{name}.F32", decode=True
            )
            assert out.tobytes() == loaded.tobytes(), name
        assert checkpoint.verify().ok
        checkpoint.close()

    tiny = np.full(
        (1, 16), np.nextafter(np.float32(0), np.float32(1)), dtype=np.float32
    )
    with np.errstate(all="ignore"):
        old_underflow = encode_nvfp4("underflow", tiny)
    assert np.frombuffer(old_underflow.companions[1].raw, dtype="<f4")[0] == 0.0
    good = cases["ties"]
    refused = [
        (good, None, {}),
        (good, -1.0, {}),
        (good, float("nan"), {}),
        (good, float("inf"), {}),
        (good, 1e300, {}),
        (good, 1.0, {}),
        (good, 6.0, {"strategy": "nvfp4-min-sse/999"}),
        (good, 6.0, {"params": {**params, "nibble_order": "high_first"}}),
        (good, 6.0, {"shape": [5, 63]}),
        (good, 6.0, {"dtype": "I8"}),
        (np.full((1, 16), np.nan, dtype=np.float32), 1.0, {}),
        (tiny, float(tiny[0, 0]), {}),
    ]
    for values, maximum, options in refused:
        try:
            encode(values, maximum, **options)
        except tensorfs.DecodeError:
            pass
        else:
            raise AssertionError(f"encoder accepted tensor_amax={maximum}, {options}")
    retry = stored(encode(good, 6.0))
    assert retry["weight"][2] == encode_nvfp4("retry", good, amax=6.0).payload
    detail = {
        "strategy": "nvfp4-min-sse/1",
        "tensorfs": tensorfs.__version__,
        "numpy": np.__version__,
        "native_sha256": hashlib.sha256(
            Path(tensorfs._ext.__file__).read_bytes()
        ).hexdigest(),
        "reference_sha256": hashlib.sha256(
            inspect.getsource(encode_nvfp4).encode()
        ).hexdigest(),
        "cases": records,
        "logical_dtypes": ["F32", "F16", "BF16"],
        "chunk_rows": [1, 2, 7],
        "writer_verified": True,
        "reference_zero_global_scale_refused": True,
        "refusals": len(refused),
    }
    return Report(
        True, len(records) * 3, len(refused), json.dumps(detail, sort_keys=True)
    )
