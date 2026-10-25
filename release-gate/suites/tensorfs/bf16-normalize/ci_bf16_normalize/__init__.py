"""Real Writer BF16 normalization, exact frozen numerical reference, bounds and resume."""

import gc
import hashlib
import itertools
import json
import math
import resource
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context, Telemetry

app = App()


class Request(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 103
    mode: str = "basic"
    runtime: bool = False


class Report(msgspec.Struct):
    passed: bool
    detail: str


def selected(key, dtype, shape):
    return {"component": "weights", "key": key, "source_dtype": dtype, "shape": list(shape)}


def rss_bytes():
    # Linux ru_maxrss is KiB, Darwin bytes. It is a process lifetime high-water mark.
    return int(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss) * (1 if sys.platform == "darwin" else 1024)


def timed(call):
    before_rss, before_cpu, started = rss_bytes(), time.process_time(), time.perf_counter()
    value = call()
    return value, {
        "wall_seconds": time.perf_counter() - started,
        "process_cpu_seconds": time.process_time() - before_cpu,
        "process_maxrss_before_bytes": before_rss,
        "process_maxrss_after_bytes": rss_bytes(),
    }


def observed(result, expected, values, np):
    assert result["relative_frobenius"] == expected["relative_frobenius"] or math.isclose(
        result["relative_frobenius"], expected["relative_frobenius"], rel_tol=2e-13, abs_tol=0
    )
    measured = result["measured"]
    for name in ("amax", "max_abs_error", "saturated"):
        assert measured[name] == expected[name], (name, measured[name], expected[name])
    for name in ("sse", "energy"):
        np.testing.assert_allclose(measured[name], expected[name], rtol=2e-13, atol=0)


def normalize(writer, source, selection, tensorfs, *, buffer=64 << 10, allowance=None, control=None):
    if allowance is None:
        allowance = sum(math.prod(row["shape"]) * 2 for row in selection)
    return writer.normalize_bf16(
        source, selection, max_buffer_bytes=buffer, max_new_bytes=allowance,
        control=control if control is not None else tensorfs.ConversionControl(),
    )


def quant_selection(key="quant.weight", dtype="F32"):
    return {
        **selected(key, "F32", (7, 256)), "dtype": dtype,
        "encoding": {"scheme": "fp8.rowwise", "params": {}},
        "strategy": "fp8-rowwise-rne/1",
    }


@app.entrypoint(accelerator=False)
def run(payload: Request, ctx: Context, tel: Telemetry) -> Report:
    import numpy as np
    import tensorfs

    from . import oracle

    assert payload.mode in ("basic", "cancel")
    with tempfile.TemporaryDirectory(prefix="cozy-bf16-normalize-") as temporary:
        directory = Path(temporary)
        if payload.mode == "cancel":
            result = cancel(directory, payload.seed, tensorfs, np, oracle)
        else:
            result = basic(directory, payload.seed, tensorfs, np, oracle, ctx, tel, payload.runtime)
        result.update({
            "mode": payload.mode, "tensorfs": tensorfs.__version__, "numpy": np.__version__,
            "native_sha256": hashlib.sha256(Path(tensorfs._ext.__file__).read_bytes()).hexdigest(),
            "oracle_sha256": hashlib.sha256(Path(oracle.__file__).read_bytes()).hexdigest(),
            "strategy": "bf16-rne/1",
            "resource_scope": "process lifetime ru_maxrss before/after each conversion; not an isolated allocation peak",
            "traffic_scope": "logical completed source reads and encoded output bytes; not physical disk or PCIe traffic",
        })
        return Report(True, json.dumps(result, sort_keys=True))


def basic(directory, seed, tensorfs, np, oracle, ctx, tel, runtime):
    rng = np.random.default_rng(seed)
    writer = tensorfs.Writer(directory / "source.cozytensors")
    component = writer.component("weights", config={"keep": "source config"})
    component.asset("keep.txt", b"source asset")
    component.put("keep", np.array([17], dtype="<i4"))
    specifications, cases = [], {}

    def plain(name, dtype, values):
        values = np.asarray(values, dtype=np.float32)
        data = oracle.from_f32(values, dtype)
        decoded = oracle.to_f32(data, dtype, list(values.shape))
        component.put(name, data, dtype=dtype, shape=values.shape)
        specifications.append(selected(name, dtype, values.shape))
        cases[name] = (decoded, len(data), values.size * (4 if dtype == "F32" else 2))

    tie_bits = np.array([
        0x3F808000, 0x3F818000, 0x3F807FFF, 0x3F808001,
        0xBF808000, 0xBF818000, 0xBF807FFF, 0xBF808001,
        0x00000000, 0x80000000, 0x00800000, 0x007FFFFF,
        0x00000001, 0x80000001, 0x00010000, 0x80010000,
    ], dtype="<u4")
    plain("f32-ties", "F32", tie_bits.view("<f4"))
    plain("f32-scalar", "F32", np.array(-1.00390625, dtype="<f4"))
    plain("f32-negative-zero-scalar", "F32", np.array(-0.0, dtype="<f4"))
    plain("f32-rank4", "F32", rng.normal(size=(2, 3, 4, 5)).astype("<f4"))
    plain("f32-positive-negative-zero", "F32", np.array([0, -0.0, 0, -0.0], dtype="<f4"))
    plain("f32-empty-leading", "F32", np.empty((0, 7), dtype="<f4"))
    plain("f32-empty-inner", "F32", np.empty((2, 0, 3), dtype="<f4"))
    plain("f16-scalar", "F16", np.array(1.00390625, dtype="<f4"))
    plain("f16-rank3", "F16", rng.normal(size=(3, 5, 7)).astype("<f4"))
    plain("f16-empty", "F16", np.empty((3, 0), dtype="<f4"))
    half_bits = np.arange(65536, dtype="<u2")
    finite_half = half_bits[(half_bits & 0x7C00) != 0x7C00].view("<f2").astype("<f4")
    plain("f16-all-finite-codes", "F16", finite_half)
    assert len(finite_half) == 63488

    rows, cols = 7, 37
    for (scheme, weight_dtype), scale_dtype, logical in itertools.product(
        (("fp8.rowwise", "F8_E4M3"), ("fp8.rowwise", "F8_E5M2"), ("int8.rowwise", "I8")),
        ("F32", "F16", "BF16"), ("F32", "F16", "BF16"),
    ):
        codes = ((np.arange(rows * cols) + seed) % 256).astype(np.uint8)
        for index, code in enumerate(codes):
            if not math.isfinite(oracle.element(int(code), weight_dtype)):
                codes[index] = 0
        scale_values = np.array([0.0, -0.0, -0.125, 0.333251953125, 1.00390625, 2.0, 32.0], dtype="<f4")
        scales = oracle.from_f32(scale_values, scale_dtype)
        scale_shape = {"F32": (rows,), "F16": (rows, 1), "BF16": (1, rows)}[logical]
        name = f"{scheme}-{weight_dtype}-{scale_dtype}-{logical}"
        component.put_encoded(
            name, dtype=logical, shape=(rows, cols), scheme=scheme, params={},
            parts={"weight": (weight_dtype, (rows, cols), codes.tobytes()),
                   "scale": (scale_dtype, scale_shape, scales)},
        )
        specifications.append(selected(name, logical, (rows, cols)))
        cases[name] = (
            oracle.rowwise(codes.tobytes(), weight_dtype, scales, scale_dtype, (rows, cols)),
            len(codes) + len(scales), rows * cols * (4 if logical == "F32" else 2),
        )
    for scheme, weight_dtype in (("fp8.rowwise", "F8_E4M3"), ("int8.rowwise", "I8")):
        for shape in ((0, 37), (3, 0)):
            name = f"empty-{scheme}-{shape[0]}-{shape[1]}"
            component.put_encoded(
                name, dtype="BF16", shape=shape, scheme=scheme, params={},
                parts={"weight": (weight_dtype, shape, b""),
                       # The format's empty matrix view has zero addressable rows.
                       "scale": ("F32", (0,), b"")},
            )
            specifications.append(selected(name, "BF16", shape))
            cases[name] = (np.empty(shape, dtype="<f4"), 0, 0)
    quant_values = rng.normal(size=(7, 256)).astype("<f4")
    component.put("quant.weight", quant_values)
    source = writer.commit()
    expected = {name: oracle.normalize(values) for name, (values, _, _) in cases.items()}
    allowance = sum(len(raw) for raw, _ in expected.values())
    actual_read = sum(item[1] for item in cases.values())
    logical_read = sum(item[2] for item in cases.values())
    reports = []
    for buffer_bytes in (16 << 10, 64 << 10):
        output = tensorfs.Writer(directory / f"normalized-{buffer_bytes}.cozytensors", base=source)
        empty = normalize(output, source, [], tensorfs, buffer=buffer_bytes, allowance=0)
        assert empty["converted_keys"] == empty["reused_keys"] == 0
        assert empty["worst_relative_frobenius"] == 0
        refused = []
        bad_shape = [dict(row) for row in specifications]
        bad_shape[-1]["shape"] = [rows, cols + 1]
        bad_dtype = [dict(row) for row in specifications]
        bad_dtype[-1]["source_dtype"] = "F32" if bad_dtype[-1]["source_dtype"] != "F32" else "F16"
        bad_key = [dict(row) for row in specifications]
        bad_key[-1]["key"] = "missing"
        extra = [dict(specifications[0], encoding={"scheme": "bf16"})]
        for label, selection, limits in (
            ("output-grant", specifications, {"allowance": allowance - 1}),
            ("workspace-grant", specifications, {"buffer": 1}),
            ("shape", bad_shape, {}), ("dtype", bad_dtype, {}), ("missing", bad_key, {}),
            ("duplicate", specifications + specifications[:1], {}), ("unknown-field", extra, {}),
        ):
            try:
                normalize(output, source, selection, tensorfs,
                          **{"buffer": buffer_bytes, "allowance": allowance, **limits})
            except (tensorfs.InvalidError, tensorfs.DiskFullError, tensorfs.NotFoundError):
                refused.append(label)
            else:
                raise AssertionError(f"accepted {label}")
            assert not any(output.has("weights", row["key"]) for row in specifications)
        control = tensorfs.ConversionControl()
        control.cancel()
        try:
            normalize(output, source, specifications, tensorfs, buffer=buffer_bytes, control=control)
        except tensorfs.NetworkError:
            pass
        else:
            raise AssertionError("pre-canceled normalization ran")
        assert control.progress() == (0, 0, 0)
        result, timing = timed(lambda: normalize(output, source, specifications, tensorfs, buffer=buffer_bytes))
        assert result["converted_keys"] == len(specifications) and result["reused_keys"] == 0
        assert result["source_bytes_read"] == actual_read
        assert result["logical_source_bytes_read"] == logical_read
        assert result["new_bytes_written"] == allowance
        assert 0 <= result["required_buffer_bytes"] <= buffer_bytes
        by_key = {item["key"]: item for item in result["tensors"]}
        assert set(by_key) == set(cases)
        for name, (values, read, _) in cases.items():
            assert by_key[name]["source_bytes_read"] == read
            assert by_key[name]["new_bytes_written"] == len(expected[name][0])
            observed(by_key[name], expected[name][1], values, np)
        repeated = normalize(output, source, specifications, tensorfs, buffer=buffer_bytes * 2, allowance=0)
        assert repeated["converted_keys"] == 0 and repeated["reused_keys"] == len(specifications)
        assert repeated["source_bytes_read"] == repeated["logical_source_bytes_read"] == repeated["new_bytes_written"] == 0
        assert repeated["worst_relative_frobenius"] is None and repeated["tensors"] == []
        checkpoint = output.commit()
        assert checkpoint.verify().ok
        assert checkpoint.components["weights"].config == {"keep": "source config"}
        assert "keep.txt" in checkpoint.components["weights"].assets
        assert checkpoint.components["weights"].tensors.read("keep").tobytes() == np.array([17], dtype="<i4").tobytes()
        for name, (want, _) in expected.items():
            value = checkpoint.components["weights"].tensors.read(name)
            assert value.dtype == "BF16" and value.shape == cases[name][0].shape
            assert value.tobytes() == want, name
        reports.append({"result": result, "timing": timing, "preflight_refusals": refused})
        checkpoint.close()
    assert reports[0]["result"]["identity"] == reports[1]["result"]["identity"]
    numerical_refusals = refusal_sources(directory, tensorfs, np, oracle)
    mixed = mixed_resume(directory, source, specifications[:2], quant_values, tensorfs, np, oracle)
    runtime_report = runtime_adapter(directory, source, specifications, cases, tensorfs, np, ctx, tel) if runtime else None
    source.close()
    return {"cases": len(cases), "reports": reports, "numerical_refusals": numerical_refusals,
            "mixed_resume": mixed, "runtime": runtime_report,
            "part_sha256": {name: hashlib.sha256(raw).hexdigest() for name, (raw, _) in expected.items()}}


def refusal_sources(directory, tensorfs, np, oracle):
    cases = {
        "nan": ("F32", np.array([np.nan], dtype="<f4")),
        "positive-inf": ("F32", np.array([np.inf], dtype="<f4")),
        "negative-inf": ("F16", np.array([-np.inf], dtype="<f4")),
        "overflow": ("F32", np.array([np.finfo(np.float32).max], dtype="<f4")),
        "subnormal-quality": ("F32", np.array([np.nextafter(np.float32(0), np.float32(1))], dtype="<f4")),
        "unsupported-plain-bf16": ("BF16", np.array([1.0], dtype="<f4")),
    }
    results = []
    for label, (dtype, values) in cases.items():
        writer = tensorfs.Writer(directory / f"refuse-source-{label}.cozytensors")
        writer.component("weights").put("value", oracle.from_f32(values, dtype), dtype=dtype, shape=values.shape)
        source = writer.commit()
        output = tensorfs.Writer(directory / f"refuse-output-{label}.cozytensors", base=source)
        try:
            normalize(output, source, [selected("value", dtype, values.shape)], tensorfs)
        except (tensorfs.DecodeError, tensorfs.UnsupportedError, tensorfs.InvalidError) as error:
            results.append({"case": label, "code": error.code, "detail": error.detail})
        else:
            raise AssertionError(f"accepted {label}")
        assert not output.has("weights", "value")
        output.abandon()
        source.close()
    for label, scheme, shape, codes, scale in (
        ("rowwise-overflow", "fp8.rowwise", (1, 4), bytes([0x40] * 4), np.finfo(np.float32).max),
        ("unsupported-tensorwise", "fp8.tensorwise", (1, 4), bytes([0x38] * 4), 1.0),
        ("unsupported-rowwise-rank3", "fp8.rowwise", (1, 1, 4), bytes([0x38] * 4), 1.0),
    ):
        writer = tensorfs.Writer(directory / f"refuse-source-{label}.cozytensors")
        writer.component("weights").put_encoded(
            "value", dtype="F32", shape=shape, scheme=scheme, params={},
            parts={"weight": ("F8_E4M3", shape, codes),
                   "scale": ("F32", (1,), np.array([scale], dtype="<f4").tobytes())},
        )
        source = writer.commit()
        output = tensorfs.Writer(directory / f"refuse-output-{label}.cozytensors", base=source)
        try:
            normalize(output, source, [selected("value", "F32", shape)], tensorfs)
        except (tensorfs.DecodeError, tensorfs.UnsupportedError, tensorfs.InvalidError) as error:
            results.append({"case": label, "code": error.code, "detail": error.detail})
        else:
            raise AssertionError(f"accepted {label}")
        assert not output.has("weights", "value")
        output.abandon()
        source.close()
    return results


def mixed_resume(directory, source, specifications, quant_values, tensorfs, np, oracle):
    path = directory / "mixed.cozytensors"
    output = tensorfs.Writer(path, base=source, resume="mixed-bf16-and-quantization")
    norm = normalize(output, source, specifications, tensorfs)
    qspec = [quant_selection()]
    quant = output.quantize(source, qspec, max_buffer_bytes=16 << 10,
                            max_new_bytes=7 * (256 + 4), control=tensorfs.ConversionControl())
    assert norm["converted_keys"] == len(specifications) and quant["encoded_keys"] == 1
    del output
    gc.collect()
    output = tensorfs.Writer(path, base=source, resume="mixed-bf16-and-quantization")
    # Changes to either journal identity refuse without invalidating the other operation.
    for label, operation in (
        ("normalization-selection", lambda: normalize(output, source, specifications[:1], tensorfs, allowance=0)),
        ("quantization-selection", lambda: output.quantize(source, [quant_selection(dtype="BF16")],
             max_buffer_bytes=16 << 10, max_new_bytes=0, control=tensorfs.ConversionControl())),
    ):
        try:
            operation()
        except tensorfs.InvalidError:
            pass
        else:
            raise AssertionError(f"resume accepted changed {label}")
    writer = tensorfs.Writer(directory / "changed-source.cozytensors", base=source)
    writer.metadata["different-source-identity"] = True
    changed = writer.commit()
    assert changed.id != source.id
    try:
        normalize(output, changed, specifications, tensorfs, allowance=0)
    except tensorfs.InvalidError:
        pass
    else:
        raise AssertionError("resume accepted changed source checkpoint")
    changed.close()
    # Opposite call order after reopen must remain valid.
    q_again = output.quantize(source, qspec, max_buffer_bytes=64 << 10,
                             max_new_bytes=0, control=tensorfs.ConversionControl())
    n_again = normalize(output, source, specifications, tensorfs, buffer=64 << 10, allowance=0)
    assert q_again["reused_keys"] == 1 and q_again["encoded_keys"] == 0
    assert n_again["reused_keys"] == len(specifications) and n_again["converted_keys"] == 0
    assert norm["identity"] == n_again["identity"] and quant["identity"] == q_again["identity"]
    checkpoint = output.commit()
    assert checkpoint.verify().ok
    stored = checkpoint.components["weights"].tensors.read("quant.weight")
    expected = tensorfs.raw.quantize("fp8.rowwise", quant_values.tobytes(), dtype="F32",
                                    shape=quant_values.shape, params={}, strategy="fp8-rowwise-rne/1")
    assert {key: value.tobytes() for key, value in stored.parts.items()} == {
        key: value.tobytes() for key, value in expected.parts.items()
    }
    checkpoint.close()
    # First binding of either operation cannot adopt a key written by the other.
    for normalization_first in (True, False):
        overlapping = tensorfs.Writer(directory / f"overlap-{normalization_first}.cozytensors", base=source)
        normalize_same = lambda: normalize(overlapping, source, [selected("quant.weight", "F32", quant_values.shape)], tensorfs)
        quantize_same = lambda: overlapping.quantize(source, qspec, max_buffer_bytes=16 << 10,
            max_new_bytes=7 * (256 + 4), control=tensorfs.ConversionControl())
        first_call, second_call = (normalize_same, quantize_same) if normalization_first else (quantize_same, normalize_same)
        first_call()
        try:
            second_call()
        except tensorfs.InvalidError:
            pass
        else:
            raise AssertionError("operation adopted a key published by the other producer")
        overlapping.abandon()
    return {"initial_normalization": norm, "initial_quantization": quant,
            "resumed_normalization": n_again, "resumed_quantization": q_again}


def runtime_adapter(directory, source, specifications, cases, tensorfs, np, ctx, tel):
    """Actual Runtime Output and write_bf16; no fake Context, Writer or numerical helper."""
    from cozy_runtime.derive import native_quantization
    from cozy_runtime.derive.quantization import Bf16Plan, Bf16Tensor, write_bf16
    from cozy_runtime.weights_store import Derivation, Output

    plan = Bf16Plan(
        components=["weights"],
        order=[("weights", key) for key in source.components["weights"].tensors],
        tensors=[Bf16Tensor(row["component"], row["key"], row["source_dtype"].lower(), tuple(row["shape"]))
                 for row in specifications],
        kept=[],
    )
    store = directory / "runtime-store"
    transaction = "sha256:" + hashlib.sha256(str(directory).encode()).hexdigest()
    grant = sum(math.prod(row["shape"]) * 2 for row in specifications) + 7 * (256 + 4)
    derivation = Derivation(base=source, order=plan.order)
    output = Output(store, "bf16", transaction, derivation, grant)
    first, timing = timed(lambda: write_bf16(source, output, ctx, tel, plan=plan))
    assert first.converted_keys == len(specifications) and first.reused_keys == 0
    assert first.source_bytes_read == sum(item[2] for item in cases.values())
    assert first.stored_source_bytes_read == sum(item[1] for item in cases.values())
    assert first.new_bytes_written == sum(math.prod(row["shape"]) * 2 for row in specifications)
    output.close()
    # A real interrupted Output reopens its native journal, even with every normalization key present.
    output = Output(store, "bf16", transaction, derivation, grant)
    replayed = write_bf16(source, output, ctx, tel, plan=plan)
    assert replayed.converted_keys == replayed.reused_keys == len(specifications)
    assert replayed.source_bytes_read == replayed.stored_source_bytes_read == replayed.new_bytes_written == 0
    assert replayed.worst_relative_frobenius is None
    conversion, pending = output.prepare_quantization(source, [quant_selection()], max_relative_frobenius=None)
    quant = native_quantization._run_native(conversion, pending, ctx, tel, stage="mixed native producer")
    assert quant["encoded_keys"] == 1
    checkpoint = output.commit()
    assert checkpoint.verify().ok
    output.close()
    # Finished reuse validates the native normalization acceptance identity before skipping work.
    output = Output(store, "bf16", transaction, derivation, grant)
    finished = write_bf16(source, output, ctx, tel, plan=plan)
    assert finished.converted_keys == finished.reused_keys == len(specifications)
    assert finished.source_bytes_read == finished.stored_source_bytes_read == finished.new_bytes_written == 0
    assert finished.worst_relative_frobenius is None
    changed = Bf16Plan(plan.components, plan.order, plan.tensors[:-1], [])
    from cozy_runtime.author import CapabilityError
    try:
        write_bf16(source, output, ctx, tel, plan=changed)
    except CapabilityError as error:
        assert error.code == "normalization_identity"
    else:
        raise AssertionError("finished Runtime output reused a changed normalization selection")
    output.close()
    checkpoint.close()
    return {"first": msgspec.to_builtins(first), "timing": timing,
            "resumed": msgspec.to_builtins(replayed), "finished": msgspec.to_builtins(finished),
            "quantization": quant}


def cancel(directory, seed, tensorfs, np, oracle):
    # Gate-owned 64MiB F32 source. Write it from a bounded 1MiB fixture window.
    shape = (16384, 1024)
    data_path = directory / "large.f32"
    window = np.tile(np.array([1.00390625, -1.01171875, 0.0, -0.0], dtype="<f4"), (256 * 1024) // 4)
    with data_path.open("wb") as handle:
        for _ in range(64):
            handle.write(window.tobytes())
    mapped = np.memmap(data_path, mode="r", dtype="<f4", shape=shape)
    writer = tensorfs.Writer(directory / "cancel-source.cozytensors")
    component = writer.component("weights")
    first = np.array([1.00390625], dtype="<f4")
    component.put("first", first)
    component.put("quant.first", np.full((1, 256), 1.00390625, dtype="<f4"))
    component.put("large", memoryview(mapped), dtype="F32", shape=shape)
    source = writer.commit()
    del mapped, window
    specifications = [selected("first", "F32", first.shape), selected("large", "F32", shape)]
    path, resume = directory / "cancel-output.cozytensors", f"bf16-cancel-{seed}"
    output = tensorfs.Writer(path, base=source, resume=resume)
    control = tensorfs.ConversionControl()
    started = time.perf_counter()
    with ThreadPoolExecutor(max_workers=1) as pool:
        future = pool.submit(normalize, output, source, specifications, tensorfs, buffer=64 << 10, control=control)
        until = time.monotonic() + 60
        while control.progress()[1] < 1 or control.progress()[0] <= first.nbytes:
            assert not future.done(), "actual cancellation window was missed"
            assert time.monotonic() < until, control.progress()
            time.sleep(0.001)
        for operation in (output.commit, output.abandon):
            try:
                operation()
            except tensorfs.ConflictError:
                pass
            else:
                raise AssertionError("busy normalization Writer operation did not refuse")
        control.cancel()
        canceled = time.perf_counter()
        try:
            future.result(timeout=60)
        except tensorfs.NetworkError as error:
            assert "stopped" in str(error)
        else:
            raise AssertionError("normalization completed before cancellation")
        cancel_join_seconds = time.perf_counter() - canceled
    assert output.has("weights", "first") and not output.has("weights", "large")
    del operation, output, future
    gc.collect()
    output = tensorfs.Writer(path, base=source, resume=resume)
    reservation = refuse_reserved(output, source, shape, tensorfs, normalization=False)
    assert output.has("weights", "first") and not output.has("weights", "large")
    result, timing = timed(lambda: normalize(output, source, specifications, tensorfs, buffer=256 << 10))
    assert result["converted_keys"] == 1 and result["reused_keys"] == 1
    assert result["worst_relative_frobenius"] is None
    assert result["source_bytes_read"] == result["logical_source_bytes_read"] == math.prod(shape) * 4
    assert result["new_bytes_written"] == math.prod(shape) * 2
    assert result["required_buffer_bytes"] <= 256 << 10
    checkpoint = output.commit()
    assert checkpoint.verify().ok
    tensors = checkpoint.components["weights"].tensors
    assert tensors.read("first").tobytes() == oracle.normalize(first)[0]
    expected_window = oracle.normalize(np.array([1.00390625, -1.01171875, 0.0, -0.0], dtype="<f4"))[0]
    block = bytearray(1 << 20)
    digest = hashlib.sha256()
    for offset in range(0, math.prod(shape) * 2, len(block)):
        tensors.read_into("large", block, offset=offset)
        assert block == expected_window * (len(block) // len(expected_window))
        digest.update(block)
    checkpoint.close()
    reverse = cancel_quantization_reservation(directory, source, shape, tensorfs)
    source.close()
    return {"normalization_reservation": reservation, "quantization_reservation": reverse,
            "fixture_source_bytes": math.prod(shape) * 4, "fixture_generated_window_bytes": 1 << 20,
            "cancel_progress": control.progress(), "cancel_join_seconds": cancel_join_seconds,
            "elapsed_to_join_seconds": canceled + cancel_join_seconds - started,
            "resumed": result, "resume_timing": timing, "output_sha256": digest.hexdigest()}


def reserved_quant_spec(key, shape):
    return {
        **selected(key, "F32", shape), "dtype": "F32",
        "encoding": {"scheme": "fp8.rowwise", "params": {}},
        "strategy": "fp8-rowwise-rne/1",
    }


def refuse_reserved(output, source, shape, tensorfs, *, normalization):
    """An unpublished key in the other operation's full plan remains reserved after reopen."""
    control = tensorfs.ConversionControl()
    assert not output.has("weights", "large")
    try:
        if normalization:
            normalize(output, source, [selected("large", "F32", shape)], tensorfs,
                      buffer=64 << 10, allowance=math.prod(shape) * 2, control=control)
        else:
            output.quantize(source, [reserved_quant_spec("large", shape)],
                            max_buffer_bytes=64 << 10,
                            max_new_bytes=shape[0] * (shape[1] + 4), control=control)
    except tensorfs.InvalidError as error:
        assert error.detail == "invalid document: selected tensor is reserved by the other conversion operation", error.detail
        detail = error.detail
    else:
        raise AssertionError("another operation took an interrupted selection's unwritten key")
    assert control.progress() == (0, 0, 0), control.progress()
    assert not output.has("weights", "large")
    return {"refused_operation": "normalize_bf16" if normalization else "quantize",
            "detail": detail, "progress": control.progress(), "unwritten_key_absent": True}


def cancel_quantization_reservation(directory, source, shape, tensorfs):
    """The opposite real interruption: FP8 owns A/B, then BF16 cannot take unwritten B."""
    path, resume = directory / "reserved-quant-output.cozytensors", "bf16-gate-quant-reservation"
    output = tensorfs.Writer(path, base=source, resume=resume)
    specs = [reserved_quant_spec("quant.first", (1, 256)), reserved_quant_spec("large", shape)]
    control = tensorfs.ConversionControl()
    with ThreadPoolExecutor(max_workers=1) as pool:
        future = pool.submit(output.quantize, source, specs, max_buffer_bytes=64 << 10,
                             max_new_bytes=260 + shape[0] * (shape[1] + 4), control=control)
        until = time.monotonic() + 60
        while control.progress()[1] < 1 or control.progress()[0] <= 256 * 4:
            assert not future.done(), "opposite-direction cancellation window was missed"
            assert time.monotonic() < until, control.progress()
            time.sleep(0.001)
        control.cancel()
        canceled = time.perf_counter()
        try:
            future.result(timeout=60)
        except tensorfs.NetworkError as error:
            assert "stopped" in str(error)
        else:
            raise AssertionError("FP8 conversion completed before cancellation")
        join_seconds = time.perf_counter() - canceled
    assert output.has("weights", "quant.first") and not output.has("weights", "large")
    del output, future
    gc.collect()
    output = tensorfs.Writer(path, base=source, resume=resume)
    result = refuse_reserved(output, source, shape, tensorfs, normalization=True)
    assert output.has("weights", "quant.first") and not output.has("weights", "large")
    output.abandon()
    return {**result, "cancel_progress": control.progress(), "cancel_join_seconds": join_seconds}
