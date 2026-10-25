"""Real CPU adapter compatibility and mixed per-component Output publication."""

import hashlib
import json
import tempfile
from pathlib import Path

import msgspec
import numpy as np
import tensorfs
from cozy_runtime.author import CapabilityError, UnsupportedInput
from cozy_runtime.derive import microscale, native_quantization
from cozy_runtime.derive.quantization import (
    ArtifactQuantizationRequest,
    prepare_source_quantization,
    quantize_component_into,
)
from cozy_runtime.weights_store import Derivation

from .oracle import encode_int8_convrot256, encode_nvfp4

ROWS, COLS = 4, 256
TOTAL_BYTES = 2 * (ROWS * COLS + ROWS * 4) + ROWS * COLS // 2 + ROWS * COLS // 16 + 4


def same(actual, expected):
    assert actual[:4] == expected[:4]
    assert actual.saturated == expected.saturated
    assert actual.max_abs_err == expected.max_abs_err
    np.testing.assert_allclose(actual.sse, expected.sse, rtol=2e-13, atol=0)
    np.testing.assert_allclose(actual.energy, expected.energy, rtol=2e-13, atol=0)


def compatibility(seed):
    rng = np.random.default_rng(seed)
    facts = []
    # These historically valid empty carriers bypass native nonempty geometry checks.
    # Nonempty calls below must still reach the native encoder and preserve its refusals.
    for cols in (0, 256):
        values = np.empty((0, cols), dtype=np.float32)
        same(
            microscale.encode_int8_convrot256("empty", values),
            encode_int8_convrot256("empty", values),
        )
        for maximum in (None, 0.0, -1.0, 6.0):
            same(
                microscale.encode_nvfp4("empty", values, maximum),
                encode_nvfp4("empty", values, maximum),
            )
        facts.append({"shape": list(values.shape), "empty_cases": 5})
    # A real strided source exercises the native immutable contiguous F32 boundary.
    values = rng.normal(size=(7, 1024)).astype("<f4")[:, ::2]
    assert not values.flags.c_contiguous
    same(
        microscale.encode_int8_convrot256("strided", values),
        encode_int8_convrot256("strided", values),
    )
    maximum = float(np.abs(values).max()) * 2
    same(
        microscale.encode_nvfp4("strided", values, maximum),
        encode_nvfp4("strided", values, maximum),
    )
    # These are genuine invalid data, not injected failures. The original native cause
    # establishes that production wrappers actually used TensorFS's measured primitive.
    refusals = []
    for scheme, producer, bad, args in (
        (
            "int8.convrot",
            microscale.encode_int8_convrot256,
            np.full((1, 256), np.nan, dtype="<f4"),
            (),
        ),
        ("nvfp4", microscale.encode_nvfp4, np.ones((1, 16), dtype="<f4"), (0.0,)),
    ):
        try:
            producer("invalid", bad, *args)
        except UnsupportedInput as error:
            assert isinstance(error.__cause__, tensorfs.DecodeError), repr(error)
            refusals.append({"scheme": scheme, "native_code": error.__cause__.code})
        else:
            raise AssertionError(f"native adapter accepted invalid {scheme} input")
    return {"empty": facts, "strided_cases": 2, "native_refusals": refusals}


def exercise(seed, ctx, tel):
    from cozy_runtime._build_provenance import COMMIT

    assert callable(tensorfs.raw.quantize_measured)
    checked = compatibility(seed)
    rng = np.random.default_rng(seed ^ 0x517)
    values = {
        ("first", "conv.weight"): rng.normal(0, 0.125, (ROWS, COLS)).astype("<f4"),
        ("first", "fp8.weight"): rng.normal(0, 0.125, (ROWS, COLS)).astype("<f4"),
        ("second", "nv.weight"): rng.normal(0, 0.125, (ROWS, COLS)).astype("<f4"),
    }
    schemes = {
        "conv.weight": "int8.convrot",
        "fp8.weight": "fp8.rowwise",
        "nv.weight": "nvfp4",
    }
    bias = np.arange(ROWS, dtype="<f4")
    with tempfile.TemporaryDirectory(prefix="cozy-adapter-output-") as directory:
        writer = tensorfs.Writer(Path(directory) / "source.cozytensors")
        for name in ("first", "second"):
            component = writer.component(name, config={"seed": seed})
            for (owner, key), value in values.items():
                if owner == name:
                    component.put(key, value)
            component.put("keep.bias", bias)
        source = writer.commit()
        try:
            selection = prepare_source_quantization(
                source, components=("first", "second"), block=COLS
            )
            selection = msgspec.structs.replace(
                selection,
                tensors=[
                    msgspec.structs.replace(row, encoding=schemes[row.key])
                    for row in selection.tensors
                ],
            )
            # FP8 forces this mixed recipe through the existing per-component route.
            assert not native_quantization.supported(source, selection, "int8.convrot")
            derivation = Derivation(base=source, order=selection.order)

            def convert(output, component, *, maximum=None):
                return quantize_component_into(
                    source,
                    output,
                    ctx,
                    ArtifactQuantizationRequest(max_relative_frobenius=maximum),
                    tel,
                    encoding="int8.convrot",
                    plan=selection,
                    component=component,
                    logical_dtype=lambda row: row.logical_dtype,
                )

            with ctx.output("tripwire", derivation) as output:
                try:
                    convert(output, "first", maximum=0.0)
                except UnsupportedInput as error:
                    assert error.code == "quantization_tripwire", error.code
                    assert output.written == 0 and not output.has(
                        "first", "conv.weight"
                    )
                    assert output.done is None
                else:
                    raise AssertionError(
                        "nonzero quantization error passed a zero bound"
                    )
            with ctx.output("bound", derivation) as output:
                convert(output, "first")
                try:
                    convert(output, "second")
                except CapabilityError as error:
                    assert error.code == "weights_output_bound", error.code
                    assert output.has("first", "conv.weight")
                    assert output.has("first", "fp8.weight")
                    assert not output.has("second", "nv.weight")
                    assert output.done is None
                else:
                    raise AssertionError(
                        "conversion exceeded its real declared output grant"
                    )
            with ctx.output("mixed", derivation) as output:
                first = convert(output, "first")
                used = output.written
                reused = convert(output, "first")
                assert reused.encoded_keys == 0 and reused.reused_keys == 2
                assert reused.source_bytes_read == reused.new_bytes_written == 0
                assert reused.worst_relative_frobenius is None
                assert output.written == used
                second = convert(output, "second")
                assert first.encoded_keys == 2 and second.encoded_keys == 1
                assert first.new_bytes_written + second.new_bytes_written == TOTAL_BYTES
                assert output.written == TOTAL_BYTES
                artifact = ctx.adopt_model(output)
            checkpoint = tensorfs.open(artifact.checkpoint, local_files_only=True)
            try:
                parts = []
                for (name, key), value in values.items():
                    item = checkpoint.components[name].tensors.read(key)
                    assert item.scheme == schemes[key] and item.dtype == "F32"
                    assert item.shape == value.shape
                    expected_parts = {
                        "weight": (
                            "F4"
                            if item.scheme == "nvfp4"
                            else "F8_E4M3"
                            if item.scheme == "fp8.rowwise"
                            else "I8",
                            value.shape,
                        ),
                        "scale": ("F8_E4M3", (ROWS, COLS // 16))
                        if item.scheme == "nvfp4"
                        else ("F32", (ROWS,)),
                    }
                    if item.scheme == "nvfp4":
                        expected_parts["scale_2"] = ("F32", ())
                    assert {
                        role: (part.dtype, part.shape)
                        for role, part in item.parts.items()
                    } == expected_parts
                    if item.scheme != "fp8.rowwise":
                        producer = (
                            encode_nvfp4
                            if item.scheme == "nvfp4"
                            else encode_int8_convrot256
                        )
                        reference = producer(key, value)
                        assert item.parts["weight"].tobytes() == reference.payload
                        for role, companion in zip(
                            ("scale", "scale_2"), reference.companions
                        ):
                            assert item.parts[role].tobytes() == companion.raw
                    else:
                        # FP8 is not being migrated; independently check its stored value law.
                        from .oracle import E4M3_MAG32

                        codes = np.frombuffer(
                            item.parts["weight"].tobytes(), dtype=np.uint8
                        ).reshape(value.shape)
                        scale = np.frombuffer(
                            item.parts["scale"].tobytes(), dtype="<f4"
                        )[:, None]
                        assert (codes & 127).max() <= 126
                        decoded = (
                            E4M3_MAG32[codes & 127]
                            * np.where(codes & 128, -1.0, 1.0)
                            * scale
                        )
                        assert (
                            np.max(np.abs(decoded - value))
                            <= np.max(np.abs(value)) * 0.125
                        )
                    parts.append(
                        {
                            "component": name,
                            "key": key,
                            "scheme": item.scheme,
                            "sha256": {
                                role: hashlib.sha256(part.tobytes()).hexdigest()
                                for role, part in item.parts.items()
                            },
                        }
                    )
                for name in ("first", "second"):
                    assert (
                        checkpoint.components[name].tensors.read("keep.bias").tobytes()
                        == bias.tobytes()
                    )
                assert checkpoint.verify().ok
                checked.update(
                    runtime_commit=COMMIT,
                    tensorfs=tensorfs.__version__,
                    seed=seed,
                    checkpoint=checkpoint.id,
                    new_bytes=TOTAL_BYTES,
                    first=msgspec.to_builtins(first),
                    second=msgspec.to_builtins(second),
                    source_counter_scope="Existing per-component counter; excludes NVFP4 amax pass and is not a physical-I/O measurement",
                    repeated_component=msgspec.to_builtins(reused),
                    tripwire_before_publication=True,
                    bound_before_second_publication=True,
                    parts=parts,
                )
            finally:
                checkpoint.close()
        finally:
            source.close()
    return artifact, json.dumps(checked, sort_keys=True)
