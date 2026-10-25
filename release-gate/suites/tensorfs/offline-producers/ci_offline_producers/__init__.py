"""Real native FP8/W4A8 conversion, frozen byte references, Writer bounds and resume."""

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
STRATEGIES = {
    "fp8.rowwise": ("fp8-rowwise-rne/1", {}),
    "w4a8.convrot": (
        "w4a8-convrot256-fixed/1",
        {"group_size": 16, "rotation": "kron_regular_h4", "rotation_size": 256},
    ),
}
ROLES = {
    "fp8.rowwise": ("scale",),
    "w4a8.convrot": ("group_scale", "scale", "codebook"),
}


class Request(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 103
    mode: str = "basic"
    runtime: bool = False


class Report(msgspec.Struct):
    passed: bool
    detail: str


def parts(encoded):
    return {k: (v.dtype, v.shape, v.tobytes()) for k, v in encoded.parts.items()}


def selection(key, dtype, shape, scheme):
    strategy, params = STRATEGIES[scheme]
    return {
        "component": "weights", "key": key, "source_dtype": dtype,
        "shape": list(shape), "dtype": dtype,
        "encoding": {"scheme": scheme, "params": params}, "strategy": strategy,
    }


def output_bytes(row):
    n, k = row["shape"]
    return n * (k + 4) if row["encoding"]["scheme"] == "fp8.rowwise" else n * (k // 2 + k // 16 + 4) + 64


@app.entrypoint(accelerator=False)
def run(payload: Request, ctx: Context) -> Report:
    import numpy as np
    import tensorfs
    from tensorfs import raw

    from . import oracle

    references = {
        "fp8.rowwise": oracle.encode_fp8_rowwise,
        "w4a8.convrot": oracle.encode_w4a8,
    }
    assert payload.mode in ("basic", "cancel-fp8", "cancel-w4a8")
    rng = np.random.default_rng(payload.seed)
    records = []
    refusals = 0

    def expected(values, scheme):
        result = references[scheme]("reference", values)
        stored = {"weight": (result.payload_dtype, tuple(result.payload_shape), result.payload)}
        for role, companion in zip(ROLES[scheme], result.companions):
            stored[role] = (companion.dtype, tuple(companion.shape), companion.raw)
        return result, stored

    def check_stats(measured, reference, values):
        assert measured["amax"] == float(np.max(np.abs(values)))
        assert measured["max_abs_error"] == reference.max_abs_err
        assert measured["saturated"] == reference.saturated
        np.testing.assert_allclose(measured["sse"], reference.sse, rtol=2e-13, atol=0)
        np.testing.assert_allclose(measured["energy"], reference.energy, rtol=2e-13, atol=0)

    def encode(values, scheme, dtype="F32", **overrides):
        strategy, params = STRATEGIES[scheme]
        options = {"dtype": dtype, "shape": values.shape, "strategy": strategy, "params": params, **overrides}
        encoded, measured = raw.quantize_measured(scheme, values.astype("<f4").tobytes(), **options)
        ordinary = raw.quantize(scheme, values.astype("<f4").tobytes(), **options)
        assert parts(ordinary) == parts(encoded)
        reference, want = expected(values, scheme)
        assert parts(encoded) == want, (scheme, values.shape)
        check_stats(measured, reference, values)
        return encoded

    with tempfile.TemporaryDirectory(prefix="cozy-offline-producers-") as temporary:
        directory = Path(temporary)
        if payload.mode != "basic":
            scheme = "fp8.rowwise" if payload.mode == "cancel-fp8" else "w4a8.convrot"
            writer = tensorfs.Writer(directory / "source.cozytensors")
            component = writer.component("weights")
            selected = []
            # Small durable first entry followed by real long conversion work.
            for key, shape in (("first", (1, 256)), ("large", (16384, 1024))):
                component.put(key, rng.normal(size=shape).astype("<f4"))
                selected.append(selection(key, "F32", shape, scheme))
            source = writer.commit()
            allowance = sum(map(output_bytes, selected))
            path = directory / "cancel.cozytensors"
            output = tensorfs.Writer(path, base=source, resume=f"offline-{payload.seed}-{scheme}")
            control = tensorfs.ConversionControl()
            with ThreadPoolExecutor(max_workers=1) as pool:
                future = pool.submit(output.quantize, source, selected,
                                     max_buffer_bytes=128 << 10, max_new_bytes=allowance, control=control)
                until = time.monotonic() + 30
                while control.progress()[1] < 1 or control.progress()[0] <= 1024:
                    assert not future.done(), "actual cancellation window was missed"
                    assert time.monotonic() < until, control.progress()
                    time.sleep(0.001)
                control.cancel()
                try:
                    future.result(timeout=30)
                except tensorfs.NetworkError as error:
                    assert "stopped" in str(error)
                else:
                    raise AssertionError("conversion completed before cancellation")
            assert output.has("weights", "first") and not output.has("weights", "large")
            del output, future
            gc.collect()
            output = tensorfs.Writer(path, base=source, resume=f"offline-{payload.seed}-{scheme}")
            changed = [dict(row) for row in selected]
            changed[0]["dtype"] = "BF16"
            try:
                output.quantize(source, changed, max_buffer_bytes=256 << 10,
                                max_new_bytes=allowance, control=tensorfs.ConversionControl())
            except tensorfs.InvalidError as error:
                assert "recipe changed" in str(error)
            else:
                raise AssertionError("resume accepted changed recipe")
            resumed = output.quantize(source, selected, max_buffer_bytes=256 << 10,
                                      max_new_bytes=allowance, control=tensorfs.ConversionControl())
            assert resumed["reused_keys"] == 1 and resumed["encoded_keys"] == 1
            assert resumed["worst_relative_frobenius"] is None
            assert resumed["source_bytes_read"] == 16384 * 1024 * 4
            assert resumed["new_bytes_written"] == output_bytes(selected[1])
            checkpoint = output.commit()
            assert checkpoint.verify().ok
            return Report(True, json.dumps({"mode": payload.mode, "resumed": resumed,
                                           "progress": control.progress(), "tensorfs": tensorfs.__version__}, sort_keys=True))

        outliers = rng.normal(size=(7, 512)).astype("<f4")
        outliers[:, 259] = 1e6
        ties = np.zeros((3, 256), dtype=np.float32)
        ties[:, :9] = [448, 1/1024, 3/1024, 1.0625, 1.1875, -1/1024, -3/1024, -1.0625, -1.1875]
        cases = {
            "zeros": np.zeros((3, 256), dtype=np.float32),
            "negative_zero": np.full((2, 256), -0.0, dtype=np.float32),
            "outliers": outliers, "ties": ties,
            "rotated_ties": (oracle.hadamard256(ties.astype(np.float64)) / 16).astype("<f4"),
            "subnormal": rng.choice([-1, 0, 1], size=(3, 256)).astype("<f4") * np.nextafter(np.float32(0), np.float32(1)),
            "floor": rng.uniform(-1e-13, 1e-13, (5, 256)).astype("<f4"),
            "wide": rng.normal(size=(9, 4096)).astype("<f4"),
            "mixed_exponents": (rng.normal(size=(17, 768)) * np.exp2(rng.integers(-90, 90, size=(17, 768)))).astype("<f4"),
        }
        writer = tensorfs.Writer(directory / "primitive.cozytensors")
        component = writer.component("weights")
        stored_references = {}
        for scheme in STRATEGIES:
            for name, values in cases.items():
                _, want = expected(values, scheme)
                for dtype in ("F32", "F16", "BF16"):
                    encoded = encode(values, scheme, dtype)
                    key = f"{scheme}.{name}.{dtype}"
                    component.put_encoded(key, dtype=dtype, shape=values.shape, scheme=scheme,
                                          params=STRATEGIES[scheme][1], parts=parts(encoded))
                    stored_references[key] = want
                for count in (1, 2, 7):
                    chunks = [parts(encode(values[start:start+count], scheme)) for start in range(0, len(values), count)]
                    for role in want:
                        if role == "codebook":
                            assert all(chunk[role][2] == want[role][2] for chunk in chunks)
                        else:
                            assert b"".join(chunk[role][2] for chunk in chunks) == want[role][2]
                if payload.runtime:
                    from cozy_runtime.derive import microscale
                    adapter = microscale.encode_fp8_rowwise if scheme == "fp8.rowwise" else microscale.encode_w4a8
                    actual, original = adapter(name, values), references[scheme](name, values)
                    assert actual[:5] == original[:5]
                    assert actual.max_abs_err == original.max_abs_err
                    np.testing.assert_allclose(actual.sse, original.sse, rtol=2e-13, atol=0)
                    np.testing.assert_allclose(actual.energy, original.energy, rtol=2e-13, atol=0)
                records.append({"scheme": scheme, "name": name, "shape": values.shape,
                                "parts": {role: hashlib.sha256(part[2]).hexdigest() for role, part in want.items()}})
            good = cases["zeros"]
            bad = [
                (good, {"strategy": STRATEGIES[scheme][0] + "unsupported"}),
                (good, {"params": {**STRATEGIES[scheme][1], "unexpected": 1}}),
                (good, {"tensor_amax": 0.0}), (good, {"dtype": "I8"}),
                (good, {"shape": [3, 128]}), (good, {"shape": [4, 256]}),
                (good, {"shape": [1, 3, 256]}), (good, {"shape": [0, 256]}),
                (np.zeros((1, 0), dtype=np.float32), {}),
                (np.full((1, 256), np.nan, dtype=np.float32), {}),
                (np.full((1, 256), np.inf, dtype=np.float32), {}),
            ]
            if scheme == "w4a8.convrot":
                bad.extend([(np.ones((1, 128), dtype=np.float32), {}),
                            (good, {"params": {**STRATEGIES[scheme][1], "group_size": 32}})])
            for values, options in bad:
                for method in (raw.quantize, raw.quantize_measured):
                    try:
                        method(scheme, values.tobytes(), **{"dtype": "F32", "shape": values.shape,
                               "strategy": STRATEGIES[scheme][0], "params": STRATEGIES[scheme][1], **options})
                    except tensorfs.DecodeError:
                        pass
                    else:
                        raise AssertionError((scheme, options))
                refusals += 1
            encode(good, scheme)
        checkpoint = writer.commit()
        assert checkpoint.verify().ok
        for key, want in stored_references.items():
            stored = checkpoint.components["weights"].tensors.read(key)
            assert parts(stored) == want
            if key.endswith(".F32"):
                output = np.empty(stored.shape, dtype="<f4")
                raw.decode_into(stored.scheme, parts(stored), output, dtype="F32", shape=stored.shape, params=stored.params)
                assert output.tobytes() == checkpoint.components["weights"].tensors.read(key, decode=True).tobytes()
        checkpoint.close()

        # The checkpoint path supplies decoded F16/BF16/F32, and stages the shared codebook once.
        writer = tensorfs.Writer(directory / "source.cozytensors")
        component = writer.component("weights")
        component.put("keep", np.array([1.0], dtype="<f4"))
        selected, originals = [], {}
        for dtype in ("F32", "F16", "BF16"):
            values = rng.normal(size=(9, 512)).astype("<f4")
            if dtype == "F16":
                data = values.astype("<f2").tobytes()
                values = np.frombuffer(data, dtype="<f2").astype("<f4").reshape(values.shape)
            elif dtype == "BF16":
                data = (values.view("<u4") >> 16).astype("<u2").tobytes()
                values = (np.frombuffer(data, dtype="<u2").astype("<u4") << 16).view("<f4").reshape(values.shape)
            else:
                data = values.tobytes()
            for scheme in STRATEGIES:
                key = f"{scheme}.{dtype}"
                component.put(key, data, dtype=dtype, shape=values.shape)
                selected.append(selection(key, dtype, values.shape, scheme))
                originals[key] = values
        source = writer.commit()
        allowance = sum(map(output_bytes, selected))
        reports = []
        for buffer_bytes in (16 << 10, 64 << 10):
            output = tensorfs.Writer(directory / f"quantized-{buffer_bytes}.cozytensors", base=source)
            for limits in ({"max_buffer_bytes": 1}, {"max_new_bytes": allowance - 1}):
                try:
                    output.quantize(source, selected, control=tensorfs.ConversionControl(),
                                    **{"max_buffer_bytes": buffer_bytes, "max_new_bytes": allowance, **limits})
                except tensorfs.DiskFullError:
                    pass
                else:
                    raise AssertionError("insufficient budget was accepted")
                assert not any(output.has("weights", row["key"]) for row in selected)
            before = tensorfs.ConversionControl()
            before.cancel()
            try:
                output.quantize(source, selected, max_buffer_bytes=buffer_bytes, max_new_bytes=allowance, control=before)
            except tensorfs.NetworkError:
                pass
            else:
                raise AssertionError("pre-canceled conversion ran")
            report = output.quantize(source, selected, max_buffer_bytes=buffer_bytes,
                                     max_new_bytes=allowance, control=tensorfs.ConversionControl())
            assert report["encoded_keys"] == 6 and report["new_bytes_written"] == allowance
            assert report["source_bytes_read"] == sum(np.prod(row["shape"]) * (4 if row["source_dtype"] == "F32" else 2) for row in selected)
            repeated = output.quantize(source, selected, max_buffer_bytes=buffer_bytes * 2,
                                       max_new_bytes=0, control=tensorfs.ConversionControl())
            assert repeated["encoded_keys"] == 0 and repeated["reused_keys"] == 6
            checkpoint = output.commit()
            assert checkpoint.verify().ok
            assert checkpoint.components["weights"].tensors.read("keep").tobytes() == np.array([1.0], dtype="<f4").tobytes()
            measured = {row["key"]: row["measured"] for row in report["tensors"]}
            for row in selected:
                key = row["key"]
                reference, want = expected(originals[key], row["encoding"]["scheme"])
                assert parts(checkpoint.components["weights"].tensors.read(key)) == want
                check_stats(measured[key], reference, originals[key])
            reports.append(report)
            checkpoint.close()
        assert reports[0]["identity"] == reports[1]["identity"]
        for row in selected[:2]:
            output = tensorfs.Writer(directory / f"quality-{row['key']}.cozytensors", base=source)
            try:
                output.quantize(source, [row], max_buffer_bytes=16 << 10, max_new_bytes=allowance,
                                max_relative_frobenius=0.0, control=tensorfs.ConversionControl())
            except tensorfs.DecodeError as error:
                assert "quantization_tripwire" in str(error)
            else:
                raise AssertionError("quality failure was published")
            assert not output.has("weights", row["key"])
            output.abandon()
        return Report(True, json.dumps({
            "mode": payload.mode, "runtime_adapters": payload.runtime,
            "tensorfs": tensorfs.__version__, "numpy": np.__version__,
            "native_sha256": hashlib.sha256(Path(tensorfs._ext.__file__).read_bytes()).hexdigest(),
            "oracle_sha256": hashlib.sha256(Path(oracle.__file__).read_bytes()).hexdigest(),
            "records": records, "refusals": refusals, "reports": reports,
        }, sort_keys=True))
