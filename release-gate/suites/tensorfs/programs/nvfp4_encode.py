"""nvfp4-encode, TensorFS's own encoder against an independent reference (`nvfp4_encode_oracle.py`), public API only
(`tensorfs.raw`, `tensorfs.Writer`, `tensorfs.Encoded`). Was the gate package `tensorfs/nvfp4-encode` (xs-174: a package can't
import TensorFS); run by `release-gate/programs.sh`."""

import hashlib
import inspect
import json
import tempfile
from pathlib import Path
from typing import TYPE_CHECKING, Unpack, cast


if TYPE_CHECKING:
    from nvfp4_encode_types import F32, CaseRecord, Options, Overrides, Parts, Refusal, Stats

SEED = 103


def run() -> dict[str, object]:
    import numpy as np
    import tensorfs
    from tensorfs import raw

    from nvfp4_encode_oracle import encode_nvfp4

    params = {
        "block_size": 16,
        "global_scale": "multiply",
        "nibble_order": "low_first",
        "scale_layout": "row_major",
    }

    measured_calls: list[Stats] = []

    def encode(values: "F32", maximum: float | None, dtype: str = "F32", **overrides: "Unpack[Overrides]") -> "tensorfs.Encoded":
        options: Options = {
            "dtype": dtype,
            "shape": values.shape,
            "strategy": "nvfp4-min-sse/1",
            "params": params,
            "tensor_amax": maximum,
        }
        options.update(overrides)  # type: ignore[typeddict-item]  # overrides: a partial Options without dtype
        data = values.astype("<f4").tobytes()
        try:
            original = raw.quantize("nvfp4", data, **options)
        except tensorfs.DecodeError as first:
            try:
                raw.quantize_measured("nvfp4", data, **options)
            except tensorfs.DecodeError as measured_error:
                assert (first.code, first.detail) == (
                    measured_error.code,
                    measured_error.detail,
                )
                raise first
            raise AssertionError("measured API accepted a refused input") from first
        measured, stats = raw.quantize_measured("nvfp4", data, **options)
        assert stored(original) == stored(measured)
        reference = encode_nvfp4("measured", values.astype("<f4"), amax=maximum)
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

    def stored(encoded: "tensorfs.Encoded") -> "Parts":
        return {
            role: (part.dtype, part.shape, part.tobytes())
            for role, part in encoded.parts.items()
        }

    rng = np.random.default_rng(SEED)
    outliers = rng.uniform(-1, 1, size=(17, 64)).astype(np.float32)
    outliers[-1, -1] = 1e6
    small = np.full(
        (3, 32), np.nextafter(np.float32(0), np.float32(1)), dtype=np.float32
    )
    small[-1, -1] = 1.0
    cases: dict[str, F32] = {
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
    records: list[CaseRecord] = []
    with tempfile.TemporaryDirectory(prefix="cozy-nvfp4-encode-") as directory:
        with tensorfs.Writer(Path(directory) / "encoded.cozytensors") as writer:
            writer.metadata["quantizer"] = "nvfp4-min-sse/1"
            component = writer.component("weights")
            references: dict[str, Parts] = {}
            maximum: float | None
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
                            for role, part in parts.items()  # type: ignore[possibly-undefined]  # the dtype loop always runs
                        },
                    }
                )
            checkpoint = writer.commit()
        for key, expected in references.items():
            result = cast("tensorfs.Encoded", checkpoint.components["weights"].tensors.read(key))  # cast: put encoded
            assert stored(result) == expected, key
        for name, values in cases.items():
            result = cast("tensorfs.Encoded", checkpoint.components["weights"].tensors.read(f"{name}.F32"))  # cast: put encoded
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
    refused: list[tuple[F32, float | None, Refusal]] = [
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
        "measured_calls": len(measured_calls),
        "measured_sse_energy_rtol": 2e-13,
        "strategy": "nvfp4-min-sse/1",
        "tensorfs": tensorfs.__version__,
        "numpy": np.__version__,
        "reference_sha256": hashlib.sha256(
            inspect.getsource(encode_nvfp4).encode()
        ).hexdigest(),
        "cases": len(records),
        "cases_sha256": hashlib.sha256(json.dumps(records, sort_keys=True).encode()).hexdigest()[:16],
        "logical_dtypes": ["F32", "F16", "BF16"],
        "chunk_rows": [1, 2, 7],
        "writer_verified": True,
        "reference_zero_global_scale_refused": True,
        "refusals": len(refused),
    }
    return detail


if __name__ == "__main__":
    import traceback

    try:
        row = {"cell": "nvfp4-encode", "result": "PASS", "detail": run()}
    except Exception as error:  # noqa: BLE001 - any failure is this program's verdict
        row = {"cell": "nvfp4-encode", "result": "FAIL", "detail": f"{type(error).__name__}: {error}"[:600]}
        traceback.print_exc()
    print("ROW " + json.dumps(row, sort_keys=True, default=str), flush=True)
