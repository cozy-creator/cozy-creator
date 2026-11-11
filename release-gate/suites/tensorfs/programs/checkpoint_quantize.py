"""checkpoint-quantize, TensorFS's own (public API only), modes basic, cancel, cancel-write. Was the gate package `tensorfs/checkpoint-quantize` (xs-174:
a package can't import TensorFS); run by `release-gate/programs.sh`."""

import gc
import hashlib
import json
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import TYPE_CHECKING, cast


if TYPE_CHECKING:
    from checkpoint_quantize_types import (
        F32,
        ArrayTensor,
        Budget,
        Conversion,
        EncodedTensor,
        Parts,
        Selection,
    )


def selection(key: str, dtype: str, shape: tuple[int, ...], scheme: str) -> "Selection":
    nvfp4 = scheme == "nvfp4"
    return {
        "component": "weights",
        "key": key,
        "source_dtype": dtype,
        "shape": list(shape),
        "dtype": dtype,
        "encoding": {
            "scheme": scheme,
            "params": (
                {
                    "block_size": 16,
                    "nibble_order": "low_first",
                    "scale_layout": "row_major",
                    "global_scale": "multiply",
                }
                if nvfp4
                else {"rotation": "kron_regular_h4", "rotation_size": 256}
            ),
        },
        "strategy": "nvfp4-min-sse/1" if nvfp4 else "int8-convrot256/1",
    }


def parts(encoded: "EncodedTensor") -> "Parts":
    return {k: (v.dtype, v.shape, v.tobytes()) for k, v in encoded.parts.items()}


def expected_bytes(row: "Selection") -> int:
    n, k = row["shape"]
    return (
        n * k + n * 4
        if row["encoding"]["scheme"] == "int8.convrot"
        else n * k // 2 + n * k // 16 + 4
    )

SEED = 103
MODES = ['basic', 'cancel', 'cancel-write']


def Report(passed: bool, *fields: object) -> dict[str, object]:  # noqa: N802 - the verdict the package returned
    """A mode's verdict: the last field is its JSON detail."""
    detail = fields[-1]
    return {"passed": passed, "detail": json.loads(detail) if isinstance(detail, str) else detail}


def run(mode: str) -> dict[str, object]:
    import numpy as np
    import tensorfs

    from checkpoint_quantize_oracle import encode_int8_convrot256, encode_nvfp4

    assert mode in ("basic", "cancel", "cancel-write")
    rng = np.random.default_rng(SEED)
    with tempfile.TemporaryDirectory(prefix="cozy-checkpoint-quantize-") as temporary:
        directory = Path(temporary)
        writer = tensorfs.Writer(directory / "source.cozytensors")
        component = writer.component("weights", config={"keep": "unchanged"})
        component.asset("label.txt", b"inherited asset")
        component.put("keep", b"\x00\x00\x80?", dtype="F32", shape=[1])
        selected: list[Selection] = []
        values: dict[str, F32] = {}
        if mode.startswith("cancel"):
            for key, shape in [("first", (1, 256)), ("large", (16384, 1024))]:
                weight = rng.normal(size=shape).astype("<f4")
                component.put(key, weight)
                selected.append(selection(key, "F32", shape, "int8.convrot"))
        else:
            for dtype in ("F32", "F16", "BF16"):
                weight = rng.normal(size=(9, 512)).astype("<f4")
                weight[-1, -1] = (
                    32.0  # Whole-tensor maximum is outside the first chunk.
                )
                if dtype == "F16":
                    stored = weight.astype("<f2").tobytes()
                    weight = (
                        np.frombuffer(stored, dtype="<f2")
                        .astype("<f4")
                        .reshape(weight.shape)
                    )
                elif dtype == "BF16":
                    stored = (weight.view("<u4") >> 16).astype("<u2").tobytes()
                    weight = (
                        (np.frombuffer(stored, dtype="<u2").astype("<u4") << 16)
                        .view("<f4")
                        .reshape(weight.shape)
                    )
                else:
                    stored = weight.tobytes()
                for scheme in ("int8.convrot", "nvfp4"):
                    key = f"{dtype}.{scheme}"
                    component.put(key, stored, dtype=dtype, shape=weight.shape)
                    selected.append(selection(key, dtype, weight.shape, scheme))
                    values[key] = weight
        source = writer.commit()
        allowance = sum(expected_bytes(row) for row in selected)
        reports: list[Conversion] = []
        if mode.startswith("cancel"):
            output = tensorfs.Writer(
                directory / "cancel.cozytensors",
                base=source,
                resume=f"cancel-{SEED}",
            )
            control = tensorfs.ConversionControl()
            with ThreadPoolExecutor(max_workers=1) as pool:
                future = pool.submit(
                    output.quantize,
                    source,
                    selected,
                    max_buffer_bytes=128 << 10,
                    max_new_bytes=allowance,
                    control=control,
                )
                until = time.monotonic() + 20
                while control.progress()[1] < 1 or (
                    control.progress()[2] <= expected_bytes(selected[0])
                    if mode == "cancel-write"
                    else control.progress()[0] <= 1024
                ):
                    assert not future.done(), (
                        "the actual cancellation window was missed"
                    )
                    assert time.monotonic() < until, control.progress()
                    time.sleep(0.001)
                for operation in (output.commit, output.abandon):
                    try:
                        operation()
                    except tensorfs.ConflictError:
                        pass
                    else:
                        raise AssertionError("busy Writer operation did not refuse")
                control.cancel()
                try:
                    future.result(timeout=20)
                except tensorfs.NetworkError as error:
                    assert "stopped" in str(error)
                else:
                    raise AssertionError("conversion completed before cancellation")
            assert output.has("weights", "first") and not output.has("weights", "large")
            del operation, future
            if mode == "cancel":
                del output
                gc.collect()
                reopened = tensorfs.Writer(
                    directory / "cancel.cozytensors",
                    base=source,
                    resume=f"cancel-{SEED}",
                )
            else:
                reopened = (
                    output  # A failed part write must also be retryable without reopen.
                )
            # An altered recipe cannot trust the first completed entry.
            changed = cast("list[Selection]", [dict(row) for row in selected])  # cast: copies to alter
            changed[0]["dtype"] = "BF16"
            try:
                reopened.quantize(
                    source,
                    changed,
                    max_buffer_bytes=256 << 10,
                    max_new_bytes=allowance,
                    control=tensorfs.ConversionControl(),
                )
            except tensorfs.InvalidError as error:
                assert "selected by another conversion" in str(error)
            else:
                raise AssertionError("resume accepted a different recipe")
            resumed = reopened.quantize(
                source,
                selected,
                max_buffer_bytes=256 << 10,
                max_new_bytes=allowance,
                control=tensorfs.ConversionControl(),
            )
            assert resumed["reused_keys"] == 1 and resumed["encoded_keys"] == 1
            assert resumed["source_bytes_read"] == 16384 * 1024 * 4
            assert resumed["new_bytes_written"] == expected_bytes(selected[1])
            assert resumed["worst_relative_frobenius"] is None
            checkpoint = reopened.commit()
            assert checkpoint.verify().ok
            return Report(
                True,
                json.dumps(
                    {
                        "mode": mode,
                        "resumed": resumed,
                        "cancel_progress": control.progress(),
                        "tensorfs": tensorfs.__version__,
                    },
                    sort_keys=True,
                ),
            )

        for buffer_bytes in (16 << 10, 64 << 10):
            output = tensorfs.Writer(
                directory / f"encoded-{buffer_bytes}.cozytensors", base=source
            )
            # Invalid budget/shape/selection refuses before any selected entry is written.
            bad = cast("list[Selection]", [dict(row) for row in selected])  # cast: copies to spoil
            bad[-1]["shape"] = [9, 256]
            for rows, limits in [
                (selected, {"max_new_bytes": allowance - 1}),
                (selected, {"max_buffer_bytes": 1}),
                (bad, {}),
                (selected + selected[:1], {}),
            ]:
                options = cast("Budget", {  # cast: limits override
                    "max_new_bytes": allowance,
                    "max_buffer_bytes": buffer_bytes,
                    **limits,
                })
                try:
                    output.quantize(
                        source, rows, control=tensorfs.ConversionControl(), **options
                    )
                except (tensorfs.InvalidError, tensorfs.DiskFullError):
                    pass
                else:
                    raise AssertionError("invalid conversion was accepted")
                assert not any(
                    output.has(row["component"], row["key"]) for row in selected
                )
            before = tensorfs.ConversionControl()
            before.cancel()
            try:
                output.quantize(
                    source,
                    selected,
                    max_buffer_bytes=buffer_bytes,
                    max_new_bytes=allowance,
                    control=before,
                )
            except tensorfs.NetworkError:
                pass
            else:
                raise AssertionError("pre-canceled conversion ran")
            assert before.progress() == (0, 0, 0)
            stats: "Conversion" = output.quantize(
                source,
                selected,
                max_buffer_bytes=buffer_bytes,
                max_new_bytes=allowance,
                control=tensorfs.ConversionControl(),
            )
            assert (
                stats["encoded_keys"] == 6 and stats["new_bytes_written"] == allowance
            )
            assert stats["source_bytes_read"] == sum(
                np.prod(row["shape"])
                * (4 if row["source_dtype"] == "F32" else 2)
                * (2 if row["encoding"]["scheme"] == "nvfp4" else 1)
                for row in selected
            )
            again = output.quantize(
                source,
                selected,
                max_buffer_bytes=buffer_bytes * 2,
                max_new_bytes=0,
                control=tensorfs.ConversionControl(),
            )
            assert again["encoded_keys"] == 0 and again["reused_keys"] == 6
            assert again["source_bytes_read"] == 0 and again["new_bytes_written"] == 0
            checkpoint = output.commit()
            assert checkpoint.components["weights"].config == {"keep": "unchanged"}
            assert (
                cast("ArrayTensor", checkpoint.components["weights"].tensors.read("keep")).tobytes()  # cast: plain
                == b"\x00\x00\x80?"
            )
            assert "label.txt" in checkpoint.components["weights"].assets
            assert checkpoint.verify().ok
            by_key = {row["key"]: row for row in stats["tensors"]}
            for row in selected:
                key = row["key"]
                reference = (
                    encode_nvfp4
                    if row["encoding"]["scheme"] == "nvfp4"
                    else encode_int8_convrot256
                )(key, values[key])
                held = parts(cast("EncodedTensor", checkpoint.components["weights"].tensors.read(key)))  # cast: put encoded
                assert held["weight"][2] == reference.payload
                for name, companion in zip(("scale", "scale_2"), reference.companions):
                    assert held[name][2] == companion.raw
                measured = by_key[key]["measured"]
                assert measured["saturated"] == reference.saturated
                assert measured["max_abs_error"] == reference.max_abs_err
                np.testing.assert_allclose(
                    measured["sse"], reference.sse, rtol=2e-13, atol=0
                )
                np.testing.assert_allclose(
                    measured["energy"], reference.energy, rtol=2e-13, atol=0
                )
            reports.append(stats)
            checkpoint.close()
        assert reports[0]["identity"] == reports[1]["identity"]
        # A real quality failure keeps the current tensor unpublished and permits clean close.
        tripwire = tensorfs.Writer(directory / "tripwire.cozytensors", base=source)
        try:
            tripwire.quantize(
                source,
                selected,
                max_buffer_bytes=16 << 10,
                max_new_bytes=allowance,
                max_relative_frobenius=0,
                control=tensorfs.ConversionControl(),
            )
        except tensorfs.DecodeError as error:
            assert "quantization_tripwire" in str(error)
        else:
            raise AssertionError("quality tripwire was ignored")
        assert not tripwire.has("weights", selected[0]["key"])
        tripwire.abandon()
        return Report(
            True,
            json.dumps(
                {
                    "mode": "basic",
                    "tensorfs": tensorfs.__version__,
                    "reports": reports,
                },
                sort_keys=True,
            ),
        )


if __name__ == "__main__":
    import traceback

    for mode in MODES:
        cell = "checkpoint-quantize" if mode == MODES[0] else f"checkpoint-quantize-{mode}"
        try:
            found = run(mode)
            row = {"cell": cell, "result": "PASS" if found["passed"] else "FAIL", "detail": found["detail"]}
        except Exception as error:  # noqa: BLE001 - any failure is this mode's verdict
            row = {"cell": cell, "result": "FAIL", "detail": f"{type(error).__name__}: {error}"[:600]}
            traceback.print_exc()
        print("ROW " + json.dumps(row, sort_keys=True, default=str), flush=True)
