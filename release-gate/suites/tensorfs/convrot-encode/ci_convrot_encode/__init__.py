"""Actual TensorFS encoding, production Runtime parity and Writer/readback through Cozy."""

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
    from tensorfs import raw

    from .oracle import encode_int8_convrot256, hadamard256

    params = {"rotation": "kron_regular_h4", "rotation_size": 256}

    measured_calls = []

    def encode(values, dtype="F32", **overrides):
        options = {
            "dtype": dtype,
            "shape": values.shape,
            "strategy": "int8-convrot256/1",
            "params": params,
        }
        options.update(overrides)
        data = values.astype("<f4").tobytes()
        try:
            original = raw.quantize("int8.convrot", data, **options)
        except tensorfs.DecodeError as first:
            try:
                raw.quantize_measured("int8.convrot", data, **options)
            except tensorfs.DecodeError as measured_error:
                assert (first.code, first.detail) == (
                    measured_error.code,
                    measured_error.detail,
                )
                raise first
            raise AssertionError("measured API accepted a refused input") from first
        measured, stats = raw.quantize_measured("int8.convrot", data, **options)
        assert stored(original) == stored(measured)
        reference = encode_int8_convrot256("measured", values.astype("<f4"))
        assert set(stats) == {"amax", "max_abs_error", "sse", "energy", "saturated"}
        assert stats["amax"] == float(np.max(np.abs(values.astype("<f4"))))
        assert stats["max_abs_error"] == reference.max_abs_err
        assert stats["saturated"] == reference.saturated
        np.testing.assert_allclose(stats["sse"], reference.sse, rtol=2e-13, atol=0)
        np.testing.assert_allclose(
            stats["energy"], reference.energy, rtol=2e-13, atol=0
        )
        measured_calls.append(stats)
        return original

    def stored(encoded):
        return {
            role: (part.dtype, part.shape, part.tobytes())
            for role, part in encoded.parts.items()
        }

    rng = np.random.default_rng(payload.seed)
    outliers = rng.normal(size=(7, 512)).astype(np.float32)
    outliers[:, 259] = 1e6
    ties = np.zeros((3, 256), dtype=np.float64)
    ties[:, :9] = [127, 0.5, 1.5, 2.5, 3.5, -0.5, -1.5, -2.5, -3.5]
    cases = {
        "zeros": np.zeros((3, 256), dtype=np.float32),
        "negative_zero": np.full((2, 256), -0.0, dtype=np.float32),
        "outliers": outliers,
        "ties": (hadamard256(ties) / 16.0).astype(np.float32),
        "subnormal": rng.choice([-1, 0, 1], size=(3, 256)).astype(np.float32)
        * np.nextafter(np.float32(0), np.float32(1)),
        "floor": rng.uniform(-1e-13, 1e-13, (5, 256)).astype(np.float32),
        "wide": rng.normal(size=(9, 4096)).astype(np.float32),
        "mixed_exponents": (
            rng.normal(size=(17, 768)) * np.exp2(rng.integers(-90, 90, size=(17, 768)))
        ).astype(np.float32),
    }
    records = []
    with tempfile.TemporaryDirectory(prefix="cozy-convrot-encode-") as directory:
        with tensorfs.Writer(Path(directory) / "encoded.cozytensors") as writer:
            writer.metadata["quantizer"] = "int8-convrot256/1"
            component = writer.component("weights")
            references = {}
            for name, values in cases.items():
                reference = encode_int8_convrot256(name, values)
                for dtype in ("F32", "F16", "BF16"):
                    result = encode(values, dtype)
                    parts = stored(result)
                    assert result.dtype == dtype and result.shape == values.shape
                    assert result.scheme == "int8.convrot" and result.params == params
                    assert parts["weight"] == ("I8", values.shape, reference.payload), (
                        name
                    )
                    assert parts["scale"] == (
                        "F32",
                        (values.shape[0],),
                        reference.companions[0].raw,
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
                        stored(encode(values[start : start + chunk_rows]))
                        for start in range(0, len(values), chunk_rows)
                    ]
                    assert (
                        b"".join(c["weight"][2] for c in chunks) == reference.payload
                    ), name
                    assert (
                        b"".join(c["scale"][2] for c in chunks)
                        == reference.companions[0].raw
                    ), name
                records.append(
                    {
                        "name": name,
                        "shape": values.shape,
                        "weight_sha256": hashlib.sha256(reference.payload).hexdigest(),
                        "scale_sha256": hashlib.sha256(
                            reference.companions[0].raw
                        ).hexdigest(),
                    }
                )
            checkpoint = writer.commit()
        for key, expected in references.items():
            result = checkpoint.components["weights"].tensors.read(key)
            assert stored(result) == expected, key
        # F32 readback must agree with the same bytes decoded through the resident API.
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

    good = cases["zeros"]
    refused = [
        (good, {"tensor_amax": 0.0}),
        (good, {"strategy": "int8-convrot256/999"}),
        (good, {"params": {**params, "rotation_size": 64}}),
        (good, {"params": {**params, "unexpected": 1}}),
        (good, {"dtype": "I8"}),
        (good, {"shape": [3, 128]}),
        (good, {"shape": [4, 256]}),
        (good, {"shape": [1, 3, 256]}),
        (good, {"shape": [0, 256]}),
        (np.full((1, 256), np.nan, dtype=np.float32), {}),
        (np.full((1, 256), np.inf, dtype=np.float32), {}),
    ]
    for values, options in refused:
        try:
            encode(values, **options)
        except tensorfs.DecodeError:
            pass
        else:
            raise AssertionError(f"encoder accepted {options}")
    retry = stored(encode(good))
    assert retry["weight"][2] == encode_int8_convrot256("retry", good).payload
    detail = {
        "measured_calls": len(measured_calls),
        "measured_sse_energy_rtol": 2e-13,
        "strategy": "int8-convrot256/1",
        "tensorfs": tensorfs.__version__,
        "numpy": np.__version__,
        "native_sha256": hashlib.sha256(
            Path(tensorfs._ext.__file__).read_bytes()
        ).hexdigest(),
        "reference_sha256": hashlib.sha256(
            inspect.getsource(encode_int8_convrot256).encode()
        ).hexdigest(),
        "rotation_reference_sha256": hashlib.sha256(
            inspect.getsource(hadamard256).encode()
        ).hexdigest(),
        "cases": records,
        "logical_dtypes": ["F32", "F16", "BF16"],
        "chunk_rows": [1, 2, 7],
        "writer_verified": True,
        "refusals": len(refused),
    }
    return Report(
        True, len(records) * 3, len(refused), json.dumps(detail, sort_keys=True)
    )
