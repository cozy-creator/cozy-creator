"""Real encoded checkpoint→checked decode→dependent CUDA graph, with retained owners."""

from __future__ import annotations

import gc
import hashlib
import importlib.metadata
import tempfile
import time
from dataclasses import asdict
from pathlib import Path

import tensorfs
import torch
from cozy_runtime.author import App, Context, Device, Invocation
from cozy_runtime.author import prepare as prepare_request
from cozy_runtime.author._invoke import run_prepared
from cozy_runtime.compute import PreparedRepeat
from tensorfs import raw, varena

from .exercise import Empty, Returned, private_input

ROWS, COLS = 128, 256
SCRATCH = 16 << 20
DTYPES = {"F32": torch.float32, "F16": torch.float16, "BF16": torch.bfloat16}


def stored(directory, scheme, seed):
    generator = torch.Generator().manual_seed(seed)
    values = torch.randint(-32, 33, (ROWS, COLS), generator=generator).float() / 32
    if scheme == "comfy.asym_w4a8_int8":
        params = {"group_size": 16, "rotation": "kron_regular_h4", "rotation_size": 256}
        relative = torch.ones((ROWS, COLS // 16), dtype=torch.float32)
        relative[0, 0] = float("nan")
        parts = {
            "weight": ("I8", (ROWS, COLS // 2), bytes(ROWS * COLS // 2)),
            "group_scale": ("F32", tuple(relative.shape), relative.numpy().tobytes()),
            "scale": ("F32", (ROWS,), torch.ones(ROWS).numpy().tobytes()),
        }
    else:
        if scheme == "nvfp4":
            strategy = "nvfp4-min-sse/1"
            params = {
                "block_size": 16,
                "global_scale": "multiply",
                "nibble_order": "low_first",
                "scale_layout": "row_major",
            }
            kwargs = {"tensor_amax": float(values.abs().max())}
        else:
            strategy = "int8-convrot256/1"
            params = {"rotation": "kron_regular_h4", "rotation_size": 256}
            kwargs = {}
        encoded = raw.quantize(
            scheme,
            values.numpy().tobytes(),
            dtype="F32",
            shape=values.shape,
            strategy=strategy,
            params=params,
            **kwargs,
        )
        parts = {
            name: (part.dtype, part.shape, part.tobytes())
            for name, part in encoded.parts.items()
        }
    writer = tensorfs.Writer(directory / "encoded.cozytensors")
    writer.component("net").put_encoded(
        "weight",
        dtype="F32",
        shape=(ROWS, COLS),
        scheme=scheme,
        params=params,
        parts=parts,
    )
    writer.component("net").put("plain", values)
    # A real supported storage format with no resident-source decoder in this candidate.
    writer.component("net").put_encoded(
        "unsupported",
        dtype="F32",
        shape=(ROWS, COLS),
        scheme="int8.rowwise",
        params={},
        parts={
            "weight": ("I8", (ROWS, COLS), bytes(ROWS * COLS)),
            "scale": ("F32", (ROWS,), torch.ones(ROWS).numpy().tobytes()),
        },
    )
    return writer.commit(), parts


def value_law(scheme, parts):
    """Independent equations with the format's F32 rounding after each value operation."""

    def view(role, dtype):
        return torch.frombuffer(bytearray(parts[role][2]), dtype=dtype).reshape(
            parts[role][1]
        )

    if scheme == "nvfp4":
        packed = torch.frombuffer(
            bytearray(parts["weight"][2]), dtype=torch.uint8
        ).reshape(ROWS, COLS // 2)
        codes = torch.empty((ROWS, COLS), dtype=torch.long)
        codes[:, 0::2], codes[:, 1::2] = packed & 15, packed >> 4
        levels = torch.tensor(
            [0, 0.5, 1, 1.5, 2, 3, 4, 6, 0, -0.5, -1, -1.5, -2, -3, -4, -6],
            dtype=torch.float64,
        )
        scale = view("scale", torch.uint8).long()
        table = torch.tensor(
            [
                mant * 2.0**-9
                if exponent == 0
                else (1 + mant / 8) * 2.0 ** (exponent - 7)
                for exponent in range(16)
                for mant in range(8)
            ],
            dtype=torch.float64,
        )
        # Native NVFP4 scale selection emits only finite positive magnitude bytes0..126.
        assert bool((scale <= 126).all())
        second = view("scale_2", torch.float32).double()
        scaled = (levels[codes] * table[scale].repeat_interleave(16, dim=1)).float()
        return (scaled.double() * second).float()
    values = view("weight", torch.int8).double().reshape(ROWS, COLS)
    scale = view("scale", torch.float32).double().reshape(ROWS, 1)
    h4 = torch.tensor(
        [[1, 1, 1, -1], [1, 1, -1, 1], [1, -1, 1, 1], [-1, 1, 1, 1]],
        dtype=torch.float64,
    )
    h256 = torch.kron(torch.kron(torch.kron(h4, h4), h4), h4)
    scaled = ((values @ h256) * scale).float()
    return (scaled.double() / 16).float()


def exercise(mode, seed, dtype, ctx):
    assert ctx.device.type == "cuda"
    if dtype not in DTYPES:
        raise ValueError("encoded gate output must be F16/BF16/F32")
    modes = {
        "encoded-nvfp4",
        "encoded-convrot",
        "encoded-cancel-before",
        "encoded-cancel-race",
        "encoded-input-error",
        "encoded-decode-error",
        "encoded-refusals",
    }
    if mode not in modes:
        raise ValueError(f"unknown encoded load mode: {mode}")
    assert callable(getattr(varena.LoadSlot, "encoded", None))
    varena.install_allocator()
    torch.cuda.set_device(0)
    torch.set_num_threads(1)
    scheme = (
        "comfy.asym_w4a8_int8"
        if mode == "encoded-decode-error"
        else "int8.convrot"
        if mode == "encoded-convrot"
        else "nvfp4"
    )
    failed_input = mode in {"encoded-input-error", "encoded-decode-error"}
    arena = varena.Arena(0)
    slot = plan = alias = execution = copied = None
    with tempfile.TemporaryDirectory(prefix="cozy-encoded-load-") as temporary:
        directory = Path(temporary)
        checkpoint, parts = stored(directory, scheme, seed)
        try:
            reference = (
                torch.zeros((ROWS, COLS), dtype=DTYPES[dtype])
                if mode == "encoded-decode-error"
                else value_law(scheme, parts).to(DTYPES[dtype])
            )
            source_bytes = sum(len(part[2]) for part in parts.values())
            final_bytes = (
                ROWS * COLS * torch.empty((), dtype=DTYPES[dtype]).element_size()
            )
            if mode == "encoded-refusals":
                failures = []
                for name, key, output_dtype, budget, message in (
                    ("plain-input", "plain", dtype, SCRATCH, "requires an encoded"),
                    ("missing-key", "absent", dtype, SCRATCH, "tensor is absent"),
                    ("output-dtype", "weight", "I8", SCRATCH, "F16, BF16 or F32"),
                    (
                        "unsupported-source",
                        "unsupported",
                        dtype,
                        SCRATCH,
                        "no resident device-input plan",
                    ),
                    ("scratch-capacity", "weight", dtype, 0, None),
                ):
                    try:
                        refused = varena.LoadSlot.encoded(
                            arena,
                            checkpoint,
                            "net",
                            key,
                            dtype=output_dtype,
                            max_scratch_bytes=budget,
                            threads=1,
                        )
                    except (ValueError, RuntimeError, tensorfs.Error) as error:
                        if message is None:
                            assert isinstance(error, torch.OutOfMemoryError), error
                        else:
                            assert message in str(error), error
                            if name == "unsupported-source":
                                assert isinstance(error, tensorfs.UnsupportedError), (
                                    error
                                )
                        failures.append(
                            {
                                "case": name,
                                "type": type(error).__name__,
                                "detail": str(error),
                            }
                        )
                    else:
                        refused.close()
                        raise AssertionError(
                            f"invalid encoded preparation accepted: {name}"
                        )
                    arena.reclaim(1 << 30)
                    assert arena.stats()["pinned_bytes"] == 0
                    assert not arena.stats()["poisoned"]
                return {
                    "mode": mode,
                    "seed": seed,
                    "device": torch.cuda.get_device_name(),
                    "refusals": failures,
                    "scope": "real native preflight refusals; no graph was captured or submitted",
                }
            source = checkpoint
            if mode == "encoded-input-error":
                copied, damaged = private_input(checkpoint, directory)
                required = raw.parts(copied, "net", "weight")["weight"]
                digest, _ = copied.chunks[required.chunk]
                damaged = (
                    directory
                    / "private-input"
                    / "chunks"
                    / digest.removeprefix("sha256:")
                )
                assert damaged.stat().st_nlink == 1
                source = copied
            slot = varena.LoadSlot.encoded(
                arena,
                source,
                "net",
                "weight",
                dtype=dtype,
                max_scratch_bytes=SCRATCH,
                threads=1,
            )
            alias = slot.tensor
            assert alias.shape == (ROWS, COLS) and alias.dtype == DTYPES[dtype]
            assert alias.device.type == "cuda" and not bool(alias.count_nonzero())
            with torch.inference_mode(False):
                output = torch.zeros((ROWS,), dtype=torch.float32, device="cuda")
            steps = 30_000 if mode == "encoded-cancel-race" else 3
            expected = reference.cuda().float().sum(dim=1).cpu() * steps
            calls = [0]

            def body(index):
                del index
                calls[0] += 1
                output.add_(alias.float().sum(dim=1))

            plan = PreparedRepeat.capture(body, state=(output,), steps=steps, load=slot)
            assert calls[0] == 2 and not bool(output.count_nonzero())
            if mode == "encoded-cancel-before":
                slot.cancel()
            if mode == "encoded-input-error":
                # Only the independent materialized copy has a broken source; shared storage stays intact.
                with damaged.open("r+b") as file:
                    file.truncate(0)
            if failed_input:
                app = App()
                submitted = []

                @app.entrypoint
                def failing(payload: Empty, ctx: Context) -> Returned:
                    submitted.append(ctx.plan(plan))
                    submitted[0].wait()
                    return Returned(True)

                _, outcome, _ = run_prepared(
                    prepare_request(app.get("failing"), {}),
                    Invocation(
                        request_id=f"encoded-error-{seed}",
                        spool=directory / "attempt",
                        deadline=time.monotonic() + 90,
                        device=Device("cuda", 0),
                    ),
                )
                assert outcome.terminal == "failed", outcome
                (execution,) = submitted
            else:
                execution = ctx.plan(plan)
            if mode == "encoded-cancel-race":
                execution.cancel()
            if mode in ("encoded-nvfp4", "encoded-convrot"):
                time.sleep(
                    3
                )  # No Python observation drives native load/decode/execute.
                assert execution.facts()["native_load"]["done"]
            if failed_input:
                try:
                    execution.wait()
                except tensorfs.Error as error:
                    codes = (
                        {"decode"}
                        if mode == "encoded-decode-error"
                        else {"io", "integrity", "not_found"}
                    )
                    assert error.code in codes, str(error)
                    result = {
                        "type": type(error).__name__,
                        "code": error.code,
                        "detail": str(error),
                    }
                else:
                    raise AssertionError("invalid owned encoded input did not fail")
                facts = execution.facts()
                assert (
                    facts["host_graph_launches"] == 0 and facts["native_load"]["failed"]
                )
                assert not bool(output.count_nonzero())
                if mode == "encoded-decode-error":
                    assert facts["native_load"]["read_bytes"] == source_bytes
                    assert facts["native_load"]["decode_completed_ns"] == 0
                    assert facts["native_load"]["decoded_bytes"] == 0
            else:
                returned = execution.wait()[0]
                progress = execution.poll()
                facts = execution.facts()
                result = {"progress": asdict(progress)}
                if mode.startswith("encoded-cancel"):
                    assert (
                        progress.done
                        and progress.canceled
                        and progress.completed_steps < steps
                    )
                    if mode == "encoded-cancel-before":
                        assert facts["host_graph_launches"] == 0
                    else:
                        assert facts["host_graph_launches"] in (0, 1)
                else:
                    assert (
                        progress.done
                        and progress.completed_steps == steps
                        and not progress.canceled
                    )
                    torch.testing.assert_close(alias.cpu(), reference, rtol=0, atol=0)
                    torch.testing.assert_close(
                        returned.cpu(), expected, rtol=1e-6, atol=1e-5
                    )
                    assert facts["host_graph_launches"] == 1
                    load = facts["native_load"]
                    assert load["read_bytes"] == source_bytes
                    assert (
                        load["decode_scheme"] == scheme
                        and load["decoded_bytes"] == final_bytes
                    )
                    assert 0 < load["scratch_bytes"] <= SCRATCH
                    assert (
                        0
                        < load["read_completed_ns"]
                        <= load["decode_completed_ns"]
                        <= load["launch_started_ns"]
                    )
                    result["output_sha256"] = hashlib.sha256(
                        returned.cpu().view(torch.uint8).numpy().tobytes()
                    ).hexdigest()
            assert calls[0] == 2 and facts["native_load"]["completion_proven"]
            assert torch.equal(
                torch.ones(16, device="cuda").add_(1).cpu(), torch.full((16,), 2.0)
            )
            plan.close()
            slot.close()
            plan = slot = None
            del execution
            if failed_input:
                submitted.clear()
            gc.collect()
            assert arena.reclaim(final_bytes) == 0, "escaped final storage lost its pin"
            del alias
            alias = None
            del body
            gc.collect()
            arena.reclaim(1 << 30)
            cleanup = arena.stats()
            assert cleanup["pinned_bytes"] == 0 and not cleanup["poisoned"], cleanup
            with varena.capture(0):
                pass
            return {
                "mode": mode,
                "seed": seed,
                "dtype": dtype,
                "scheme": scheme,
                "source_bytes": source_bytes,
                "final_bytes": final_bytes,
                "scratch_budget": SCRATCH,
                "runtime": importlib.metadata.version("cozy-runtime"),
                "tensorfs": tensorfs.__version__,
                "device": torch.cuda.get_device_name(),
                "native": facts,
                "result": result,
                "cleanup": cleanup,
                "python_body_calls": calls[0],
            }
        finally:
            if plan is not None:
                plan.close()
            if slot is not None:
                slot.close()
            if copied is not None:
                copied.close()
            checkpoint.close()
