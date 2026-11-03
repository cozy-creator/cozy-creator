"""Real standalone versus supplied-storage block regression for the connected decoder."""

from __future__ import annotations

import gc
import hashlib
import json
import sys
import tempfile
import traceback
import weakref
from contextlib import contextmanager
from pathlib import Path

import torch
from cozy_runtime.author import Context, denoise_loop
from cozy_runtime.graphs.domain import Domain
from cozy_runtime.internal import prebuilt, triton_kernels, verify
from cozy_runtime.internal.execution import native_vae, phase
from torch.utils._python_dispatch import TorchDispatchMode

from . import Probe, Report, actual, app, readiness, settle
from .model import Loaded, selected
from .model import rotary as make_rotary

SHAPE = (3, 1797, 2048)
_UNPROVEN = []


@contextmanager
def source_calls(module):
    """Observe real Python source callbacks; never replace hooks or their allowlist."""
    pre = {fn.__code__ for fn in module._forward_pre_hooks.values()}
    post = {fn.__code__ for fn in module._forward_hooks.values()}
    counts = {"pre": 0, "post": 0}
    prior = sys.getprofile()

    def observe(frame, event, arg):
        if event == "call":
            target = frame.f_locals.get("_")
            if frame.f_code in pre and target is module:
                counts["pre"] += 1
            if (
                frame.f_code in post
                and isinstance(target, tuple)
                and target
                and target[0] is module
            ):
                counts["post"] += 1
        if prior is not None:
            prior(frame, event, arg)

    sys.setprofile(observe)
    try:
        yield counts
    finally:
        sys.setprofile(prior)


class Census(TorchDispatchMode):
    def __init__(self, vt, device):
        super().__init__()
        self.vt, self.device, self.rows = vt, device, []
        self.inference, self.grad = (
            torch.is_inference_mode_enabled(),
            torch.is_grad_enabled(),
        )

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        before = self.vt.stats(self.device)
        leaf = self.vt.split_context(self.device)["reason"] == "leaf_active"
        with torch.inference_mode(self.inference), torch.set_grad_enabled(self.grad):
            value = func(*args, **(kwargs or {}))
        after = self.vt.stats(self.device)
        self.rows.append(
            {
                "op": str(func),
                "leaf": leaf,
                "allocations": after["allocations"] - before["allocations"],
            }
        )
        return value


def compare(value, reference):
    error = verify.error(torch, value.cpu(), reference)
    assert error <= prebuilt.BUDGET
    return {
        "relative_l2": error,
        "bound": prebuilt.BUDGET,
        "bit_exact": torch.equal(value.cpu(), reference),
    }


def baseline(before, after):
    for key in (
        "allocated_bytes",
        "allocated_capacity",
        "mapped_bytes",
        "pending_bytes",
        "pools",
        "pools_bytes",
        "event_handles",
    ):
        assert before[key] == after[key], (key, before[key], after[key])


def joined_producer(
    prepared, variant, loaded, x, rotary, marker, reference, request, vt
):
    """Actual producer T is joined to S; live owners last through checked S completion."""
    from cozy_runtime.internal import accel

    on, device = x.device, x.device.index or 0
    stream = torch.cuda.current_stream(device)
    assert stream.cuda_stream == prepared.stream
    producer = accel.nonblocking_stream(torch, device)
    parent_ready, producer_ready = torch.cuda.Event(), torch.cuda.Event()
    parent_ready.record(stream)
    held = {}
    owned = []
    report = {
        "kind": "joined-producer",
        "producer_stream": producer.cuda_stream,
        "execution_stream": stream.cuda_stream,
        "GPU_qualified": False,
    }
    invocation = buffers = values = tables = output = None
    try:
        producer.wait_event(parent_ready)
        allocations = [
            x.numel() * x.element_size(),
            *(t.numel() * t.element_size() for t in rotary),
            *variant.allocations,
        ]
        key = (
            "fixture.vae.joined-bootstrap/1:"
            + hashlib.sha256(
                repr((prepared.metadata.source, allocations)).encode()
            ).hexdigest()
        )
        request.memory.remember_working_phase(key, (request,))
        with torch.cuda.stream(producer):
            selection = vt.split_select(
                device,
                key,
                [(variant.identity, allocations)],
                preferred=variant.identity,
                stream=producer.cuda_stream,
            )
            token = phase.begin_split(
                device, key, variant.identity, selection, stream=producer.cuda_stream
            )
            submitted = False
            try:
                values, tables = x.clone(), tuple(t.clone() for t in rotary)
                target = torch.empty(SHAPE, dtype=torch.float32, device=on)
                buffers = prepared.allocate_buffers(variant.identity, target)
                submitted = True
            finally:
                report["allocation_only_bootstrap"] = vt.split_end(
                    device, token, submitted=submitted
                )
            producer_ready.record(producer)
        # The allocation-only scope has no eager owner and is never whole-decoder demand.
        assert not report["allocation_only_bootstrap"]["published"]
        report["producer_complete_before_join"] = producer_ready.query()
        stream.wait_event(producer_ready)
        report["producer_event_joined_before_bind"] = True
        invocation = prepared.bind(values, tables, variant.identity, buffers)
        owned = [
            values,
            *tables,
            buffers.output,
            buffers.packed,
            buffers.expanded,
            *buffers.attention,
        ]
        owned += [t for t in (buffers.weight, buffers.workspace) if t is not None]
        references = [weakref.ref(t) for t in owned]

        def consume(i, marker):
            del i, marker
            with source_calls(loaded.block) as hooks:
                held["output"] = invocation()
            held["hooks"] = dict(hooks)
            held["owners_alive_after_submit"] = all(
                ref() is not None for ref in references
            )

        denoise_loop(
            consume, range(1), state=(marker,), inputs=(), key=("joined-T-to-S", key)
        )
        assert stream.query() and all(ref() is not None for ref in references)
        output = held.pop("output")
        assert (
            output is buffers.output
            and invocation.calls == 1
            and held["hooks"] == {"pre": 1, "post": 1}
        )
        report.update(
            held,
            checked_S_completion=True,
            owners_retained_through_completion=True,
            **compare(output, reference),
            key=key,
            GPU_qualified=True,
        )
        output = invocation = buffers = values = tables = target = consume = None
        owned.clear()
        held.clear()
        gc.collect()
        stream.synchronize()
        producer.synchronize()
        vt.trim(device)
        return report
    finally:
        # Retire only after both actual streams are drained and T tensor owners drop.
        try:
            stream.synchronize()
            producer.synchronize()
        except BaseException:
            _UNPROVEN.append(
                (
                    producer,
                    stream,
                    prepared,
                    x,
                    rotary,
                    values,
                    tables,
                    buffers,
                    invocation,
                    output,
                    owned,
                    held,
                )
            )
            raise
        output = invocation = buffers = values = tables = target = None
        owned.clear()
        held.clear()
        gc.collect()
        vt.trim(device)
        vt.retire_stream(producer, device=on)
        accel.destroy_stream(torch, producer)


def cases(payload, request, vt, on, backend):
    device = on.index or 0
    stream = torch.cuda.current_stream(device)
    result = {
        "cases": [],
        "keys": [],
        "scope": "standalone/supplied block regression; does not qualify the connected full decoder",
    }
    initial = vt.stats(device)
    initial_weights = request.memory.native.stats()
    loaded = None
    try:
        generator = torch.Generator().manual_seed(payload.seed)
        rotary = make_rotary(on, SHAPE[1])
        marker = torch.zeros(1, dtype=torch.int64, device=on)
        for storage in (torch.float16, torch.float32):
            with tempfile.TemporaryDirectory(prefix="vae-supplied-") as folder:
                loaded = Loaded(request, Path(folder), storage, backend)
                for residual in (torch.float16, torch.float32):
                    x = (torch.randn(SHAPE, generator=generator) * 0.1).to(on, residual)
                    expected = {
                        count: loaded.expected(x, rotary, count) for count in (3, 2)
                    }
                    control = {}

                    def standalone(
                        i,
                        marker,
                        values,
                        cosine,
                        sine,
                        *,
                        loaded=loaded,
                        control=control,
                    ):
                        del i, marker
                        with source_calls(loaded.block) as hooks:
                            control["output"] = loaded.run(values, (cosine, sine))
                        control["hooks"] = dict(hooks)

                    denoise_loop(
                        standalone,
                        range(1),
                        state=(marker,),
                        inputs=(x, *rotary),
                        key=(
                            "standalone-supplied-control",
                            str(storage),
                            str(residual),
                        ),
                    )
                    records = [
                        r
                        for r in request.working_splits.values()
                        if r.get("site") == "fused-layer-scale-block"
                    ]
                    record = records[-1]
                    actual_chunk = record["batch_chunk"]
                    reference = expected.get(actual_chunk)
                    if reference is None:
                        reference = loaded.expected(x, rotary, actual_chunk)
                    assert control["hooks"] == {"pre": 1, "post": 1}
                    result["cases"].append(
                        {
                            "kind": "standalone",
                            "storage": str(storage),
                            "residual": str(residual),
                            "hooks": control["hooks"],
                            "record": dict(record),
                            **compare(control["output"], reference),
                        }
                    )
                    standalone_host = control.pop("output").cpu()
                    control.clear()
                    for chunk in (3, 2):
                        with source_calls(loaded.block) as hooks:
                            metadata = native_vae.describe_block(
                                loaded.block, SHAPE, residual, on
                            )
                            prepared = native_vae.prepare_block(
                                metadata, chunks=(chunk,)
                            )
                        assert hooks == {"pre": 0, "post": 0}
                        variant = prepared.variants[0]
                        key = (
                            "fixture.vae.bootstrap/1:"
                            + hashlib.sha256(
                                repr(
                                    (
                                        metadata.source,
                                        str(residual),
                                        chunk,
                                        variant.allocations,
                                    )
                                ).encode()
                            ).hexdigest()
                        )
                        request.memory.remember_working_phase(key, (request,))
                        result["keys"].append(key)
                        held = {}

                        def allocate(i, marker, values):
                            del i, marker, values
                            selection = vt.split_select(
                                device,
                                key,
                                [(variant.identity, variant.allocations)],
                                preferred=variant.identity,
                                stream=stream.cuda_stream,
                            )
                            token = phase.begin_split(
                                device,
                                key,
                                variant.identity,
                                selection,
                                stream=stream.cuda_stream,
                            )
                            submitted = False
                            try:
                                output = torch.empty(
                                    SHAPE, dtype=torch.float32, device=on
                                )
                                held["buffers"] = prepared.allocate_buffers(
                                    variant.identity, output
                                )
                                held["selection"] = selection
                                held["allocation_stream"] = torch.cuda.current_stream(
                                    device
                                ).cuda_stream
                                submitted = True
                            finally:
                                held["bootstrap"] = vt.split_end(
                                    device, token, submitted=submitted
                                )

                        denoise_loop(
                            allocate,
                            range(1),
                            state=(marker,),
                            inputs=(x,),
                            key=("allocation-only-bootstrap", key),
                        )
                        buffers = held.pop("buffers")
                        invocation = prepared.bind(x, rotary, variant.identity, buffers)
                        census = Census(vt, device)

                        def supplied(
                            i,
                            marker,
                            values,
                            cosine,
                            sine,
                            *,
                            census=census,
                            loaded=loaded,
                            held=held,
                            invocation=invocation,
                        ):
                            del i, marker, values, cosine, sine
                            with census, source_calls(loaded.block) as hooks:
                                held["output"] = invocation()
                            held["hooks"] = dict(hooks)

                        before = vt.stats(device)
                        denoise_loop(
                            supplied,
                            range(1),
                            state=(marker,),
                            inputs=(x, *rotary),
                            key=("supplied-body", key),
                        )
                        after = vt.stats(device)
                        output = held.pop("output")
                        assert output is buffers.output and invocation.calls == 1
                        assert (
                            held["allocation_stream"]
                            == prepared.stream
                            == stream.cuda_stream
                        )
                        assert stream.query()
                        assert held["hooks"] == {"pre": 1, "post": 1}
                        assert after["allocations"] == before["allocations"]
                        assert not any(
                            row["leaf"] and row["allocations"] for row in census.rows
                        )
                        supplied_records = [
                            r
                            for r in request.working_splits.values()
                            if r.get("site") == "supplied-layer-scale-block"
                        ]
                        supplied_record = supplied_records[-1]
                        assert (
                            supplied_record["additional_allocations"] == ()
                            and supplied_record["invocations"] == 1
                        )
                        result["cases"].append(
                            {
                                "kind": "supplied",
                                "storage": str(storage),
                                "residual": str(residual),
                                "chunk": chunk,
                                "hooks": held["hooks"],
                                "metadata_hooks": dict(hooks),
                                "bootstrap": held["bootstrap"],
                                "record": dict(supplied_record),
                                "census": census.rows,
                                "versus_standalone_relative_l2": verify.error(
                                    torch, output.cpu(), standalone_host
                                ),
                                **compare(output, expected[chunk]),
                            }
                        )
                        try:
                            invocation()
                            raise AssertionError("supplied invocation replayed")
                        except RuntimeError as error:
                            assert "one-shot" in str(error)
                        output = invocation = buffers = supplied = allocate = None
                        held.clear()
                        settle(vt, device)
                        if (
                            chunk == 2
                            and storage is torch.float16
                            and residual is torch.float16
                        ):
                            joined = joined_producer(
                                prepared,
                                variant,
                                loaded,
                                x,
                                rotary,
                                marker,
                                expected[2],
                                request,
                                vt,
                            )
                            result["cases"].append(joined)
                            result["keys"].append(joined["key"])
                        prepared = metadata = None
                    x = expected = standalone_host = reference = standalone = None
                    settle(vt, device)
                loaded.close()
                loaded = None
                request.memory.native.reclaim(
                    request.memory.native.stats()["reclaimable_bytes"]
                )
        marker = rotary = None
        settle(vt, device)
        result["final"] = vt.stats(device)
        result["final_weights"] = request.memory.native.stats()
        baseline(initial, result["final"])
        for key in ("resident_bytes", "reserved_bytes", "pinned_bytes"):
            assert initial_weights[key] == result["final_weights"][key]
        assert len(result["cases"]) == 13
        result["passed"] = True
    except Exception:  # noqa: BLE001 - preserve the real fixture failure in its receipt
        result["passed"] = False
        result["error"] = traceback.format_exc()
    finally:
        if loaded is not None:
            try:
                loaded.close()
            except Exception:  # noqa: BLE001 - preserve the real fixture failure in its receipt
                result["cleanup_error"] = traceback.format_exc()
    return result


@app.entrypoint(accelerator=True)
def consumer(payload: Probe, ctx: Context) -> Report:
    request, vt, provenance = actual(payload, ctx)
    device, on = ctx.device.index or 0, request.memory.device
    torch.set_num_threads(1)
    startup = readiness(vt, device)
    ready = torch.cuda.Event()
    ready.record(torch.cuda.current_stream(device))
    ready.synchronize()
    vt.trim(device)
    before = vt.stats(device)
    domain = Domain(torch, request.memory)
    result = {"passed": False}
    try:
        domain.initialize()
        domain.stream.wait_event(ready)
        with (
            torch.cuda.stream(domain.stream),
            domain.using(),
            torch.no_grad(),
            torch.autocast("cuda", dtype=torch.float16),
        ):
            domain.prime_blas()
            backend, attention_ready = selected(on)
            from minimax_h3.vae_tiles import VAE_BLOCK

            triton_kernels.load(VAE_BLOCK.kernels, device=on)
            settle(vt, device)
            result = cases(payload, request, vt, on, backend)
            result["attention_artifact"] = attention_ready.artifact
    except Exception:  # noqa: BLE001 - preserve the real fixture failure in its receipt
        result["error"] = traceback.format_exc()
    finally:
        result.update(
            provenance=provenance, startup_readiness=startup, pre_domain=before
        )
        try:
            domain.finish()
            vt.trim(device)
            result["post_domain"] = vt.stats(device)
            baseline(before, result["post_domain"])
        except Exception:  # noqa: BLE001 - preserve the real fixture failure in its receipt
            result["passed"] = False
            result["domain_cleanup_error"] = traceback.format_exc()
    return Report(result["passed"], json.dumps(result, sort_keys=True))
