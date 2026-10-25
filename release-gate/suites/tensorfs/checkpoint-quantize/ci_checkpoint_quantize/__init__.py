"""Real bounded checkpoint conversion, exact production bytes and Writer resume."""

import gc
import hashlib
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
    detail: str


def selection(key, dtype, shape, scheme):
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


def parts(encoded):
    return {k: (v.dtype, v.shape, v.tobytes()) for k, v in encoded.parts.items()}


def expected_bytes(row):
    n, k = row["shape"]
    return (
        n * k + n * 4
        if row["encoding"]["scheme"] == "int8.convrot"
        else n * k // 2 + n * k // 16 + 4
    )


@app.entrypoint(accelerator=False)
def run(payload: Request, ctx: Context) -> Report:
    import numpy as np
    import tensorfs
    from cozy_runtime.derive.microscale import encode_int8_convrot256, encode_nvfp4

    assert payload.mode in ("basic", "cancel", "cancel-write")
    rng = np.random.default_rng(payload.seed)
    with tempfile.TemporaryDirectory(prefix="cozy-checkpoint-quantize-") as temporary:
        directory = Path(temporary)
        writer = tensorfs.Writer(directory / "source.cozytensors")
        component = writer.component("weights", config={"keep": "unchanged"})
        component.asset("label.txt", b"inherited asset")
        component.put("keep", b"\x00\x00\x80?", dtype="F32", shape=[1])
        selected, values = [], {}
        if payload.mode.startswith("cancel"):
            for key, shape in [("first", (1, 256)), ("large", (16384, 1024))]:
                source = rng.normal(size=shape).astype("<f4")
                component.put(key, source)
                selected.append(selection(key, "F32", shape, "int8.convrot"))
        else:
            for dtype in ("F32", "F16", "BF16"):
                source = rng.normal(size=(9, 512)).astype("<f4")
                source[-1, -1] = (
                    32.0  # Whole-tensor maximum is outside the first chunk.
                )
                if dtype == "F16":
                    stored = source.astype("<f2").tobytes()
                    source = (
                        np.frombuffer(stored, dtype="<f2")
                        .astype("<f4")
                        .reshape(source.shape)
                    )
                elif dtype == "BF16":
                    stored = (source.view("<u4") >> 16).astype("<u2").tobytes()
                    source = (
                        (np.frombuffer(stored, dtype="<u2").astype("<u4") << 16)
                        .view("<f4")
                        .reshape(source.shape)
                    )
                else:
                    stored = source.tobytes()
                for scheme in ("int8.convrot", "nvfp4"):
                    key = f"{dtype}.{scheme}"
                    component.put(key, stored, dtype=dtype, shape=source.shape)
                    selected.append(selection(key, dtype, source.shape, scheme))
                    values[key] = source
        source = writer.commit()
        allowance = sum(expected_bytes(row) for row in selected)
        reports = []
        if payload.mode.startswith("cancel"):
            output = tensorfs.Writer(
                directory / "cancel.cozytensors",
                base=source,
                resume=f"cancel-{payload.seed}",
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
                    if payload.mode == "cancel-write"
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
            if payload.mode == "cancel":
                del output
                gc.collect()
                reopened = tensorfs.Writer(
                    directory / "cancel.cozytensors",
                    base=source,
                    resume=f"cancel-{payload.seed}",
                )
            else:
                reopened = (
                    output  # A failed part write must also be retryable without reopen.
                )
            # An altered recipe cannot trust the first completed entry.
            changed = [dict(row) for row in selected]
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
                assert "recipe changed" in str(error)
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
                        "mode": payload.mode,
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
            bad = [dict(row) for row in selected]
            bad[-1]["shape"] = [9, 256]
            for rows, limits in [
                (selected, {"max_new_bytes": allowance - 1}),
                (selected, {"max_buffer_bytes": 1}),
                (bad, {}),
                (selected + selected[:1], {}),
            ]:
                options = {
                    "max_new_bytes": allowance,
                    "max_buffer_bytes": buffer_bytes,
                    **limits,
                }
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
            stats = output.quantize(
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
                checkpoint.components["weights"].tensors.read("keep").tobytes()
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
                stored = parts(checkpoint.components["weights"].tensors.read(key))
                assert stored["weight"][2] == reference.payload
                for name, companion in zip(("scale", "scale_2"), reference.companions):
                    assert stored[name][2] == companion.raw
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
                    "native_sha256": hashlib.sha256(
                        Path(tensorfs._ext.__file__).read_bytes()
                    ).hexdigest(),
                },
                sort_keys=True,
            ),
        )
