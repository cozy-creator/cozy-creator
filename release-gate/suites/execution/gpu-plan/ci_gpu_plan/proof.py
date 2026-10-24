"""Real CUDA: a resident two-branch denoiser, explicit noise/guidance, Euler recurrence.

This is a model-shaped execution/ownership proof, not full H3 qualification. No mock backend.
"""
import gc
import hashlib
import importlib.metadata
import json
import sys
import threading
import time
from pathlib import Path

import torch
from tensorfs import varena
from cozy_runtime.author import Context, Device, PreparedRepeat


def report(**fields):
    print(json.dumps(fields), flush=True)


def main():
    seed = int(sys.argv[1])
    varena.install_allocator()
    torch.cuda.set_device(0)
    torch.backends.cuda.matmul.allow_tf32 = False
    gen = torch.Generator(device="cuda").manual_seed(seed)
    initial = torch.randn((64, 256), device="cuda", generator=gen) * 0.1
    source_a = torch.randn((256, 512), device="cuda", generator=gen) * 0.02
    source_b = torch.randn((512, 256), device="cuda", generator=gen) * 0.02
    conditioning = torch.randn((64, 512), device="cuda", generator=gen) * 0.02
    noise = torch.randn((30, 64, 256), device="cuda", generator=gen)
    sigmas = torch.linspace(0.02, 0.001, 30, device="cuda")
    guidance = torch.linspace(1.0, 2.0, 30, device="cuda")
    noise_digest = hashlib.sha256(noise.cpu().numpy().tobytes()).hexdigest()

    arena = varena.Arena(0)
    region = arena.reserve(source_a.nbytes + source_b.nbytes)
    fill = region.prepare()
    fill.tensor.zero_()
    flat = fill.tensor.view(torch.float32)
    weight_a = flat[:source_a.numel()].view_as(source_a)
    weight_b = flat[source_a.numel():].view_as(source_b)
    weight_a.copy_(source_a)
    weight_b.copy_(source_b)
    fill.commit()
    del flat, fill
    latent = initial.clone()
    callback_calls = [0]

    def arithmetic(value, index):
        shared = value @ weight_a
        unconditional = torch.tanh(shared) @ weight_b
        conditional = torch.tanh(shared + conditioning) @ weight_b
        scale = guidance.index_select(0, index).reshape(1, 1)
        sigma = sigmas.index_select(0, index).reshape(1, 1)
        explicit_noise = noise.index_select(0, index).squeeze(0)
        prediction = unconditional + scale * (conditional - unconditional)
        value.add_(-sigma * prediction + sigma * explicit_noise * 0.01)

    reference = initial.clone()
    for i in range(30):
        arithmetic(reference, torch.tensor([i], dtype=torch.int64, device="cuda"))
    torch.cuda.synchronize()

    def step(index):
        callback_calls[0] += 1
        arithmetic(latent, index)

    prepared = PreparedRepeat.capture(step, state=(latent,), steps=30)
    assert callback_calls[0] == 2, callback_calls
    ctx = Context("gpu-plan-proof", time.monotonic() + 120, Device("cuda", 0))
    execution = ctx.plan(prepared)
    assert execution is not None
    # No poll/wait/launch/callback during this interval: the submission thread is parked.
    time.sleep(0.75)
    status = execution.poll()
    assert status.done and status.completed_steps == 30 and not status.canceled, status
    outputs = execution.wait()
    assert callback_calls[0] == 2, "Python drove a repeated step"
    torch.testing.assert_close(outputs[0], reference, rtol=1e-5, atol=1e-6)
    first = outputs[0].clone()
    max_error = float((outputs[0] - reference).abs().max())
    report(stage="resident-30", callback_calls=callback_calls[0], steps=status.completed_steps,
           body_nodes=prepared._native.body_nodes, max_error=max_error)

    # Reuse with a new producer stream, delayed old outputs, and stale control handles.
    alternate = torch.cuda.Stream()
    with torch.cuda.stream(alternate):
        latent.copy_(initial)
        again = prepared.submit()
    for name in ("cancel", "close"):
        try:
            getattr(execution, name)()
        except RuntimeError:
            pass
        else:
            raise AssertionError(f"stale {name} was accepted")
    second = again.wait()[0]
    torch.testing.assert_close(second, reference, rtol=1e-5, atol=1e-6)
    torch.testing.assert_close(execution.wait()[0], first, rtol=0, atol=0)
    assert callback_calls[0] == 2, "reuse recaptured the body"
    assert arena.reclaim(region.allocated_bytes) == 0, "graph lost weight owners"
    report(stage="reuse-owned-output", stale_handles="refused", callback_calls=callback_calls[0])

    # Standard seeded RNG is rejected before its op executes; no generator replay prologue is bypassed.
    try:
        PreparedRepeat.capture(lambda index: latent.add_(torch.randn_like(latent)), state=(latent,), steps=2)
    except RuntimeError as error:
        assert "explicit noise" in str(error), str(error)
    else:
        raise AssertionError("captured RNG was accepted")

    # Geometry/storage edits are refused before any launch of the stale definition.
    geometry = initial.clone()
    metadata_plan = PreparedRepeat.capture(lambda index: geometry.mul_(0.99), state=(geometry,), steps=2)
    geometry.resize_(128, 256)
    try:
        metadata_plan.submit()
    except RuntimeError as error:
        assert "geometry changed" in str(error), str(error)
    else:
        raise AssertionError("changed external storage was accepted")
    metadata_plan.close()

    # Real event nodes are not legal in this conditional-body contract.
    event = torch.cuda.Event(external=True)
    rejected = initial.clone()
    def external_event(index):
        rejected.add_(0.1)
        event.record()
    try:
        PreparedRepeat.capture(external_event, state=(rejected,), steps=2, owners=(event,))
    except torch.AcceleratorError as error:
        assert getattr(error, "error_code", None) == 801, repr(error)
    else:
        raise AssertionError("unsupported external event node was accepted")
    report(stage="eligibility", rng="refused", metadata="refused", external_event="refused")

    # A long real GEMM loop exercises asynchronous progress and cancellation during native wait.
    long_state = initial.clone()
    def long_step(index):
        long_state.add_(torch.tanh(long_state @ weight_a) @ weight_b * 0.00001)
    long_plan = PreparedRepeat.capture(long_step, state=(long_state,), steps=100000)
    long_run = long_plan.submit()
    joined = []
    waiter = threading.Thread(target=lambda: joined.append(long_run.wait()))
    waiter.start()
    time.sleep(0.025)
    began = time.monotonic()
    long_run.cancel()
    waiter.join(timeout=30)
    assert not waiter.is_alive(), "cancellation blocked behind native wait"
    stopped = long_run.poll()
    assert stopped.done and stopped.canceled and 0 < stopped.completed_steps < 100000, stopped
    report(stage="cancel", completed=stopped.completed_steps, seconds=time.monotonic() - began)
    long_run.close()
    again.close()
    # Release all caller aliases too; native graph close alone must not invalidate them.
    del weight_a, weight_b, arithmetic, step, long_step
    gc.collect()
    released = arena.reclaim(region.allocated_bytes)
    assert released >= region.allocated_bytes, (released, arena.stats())
    import cozy_runtime._kernels._execution as native
    report(result={
        "passed": True, "seed": seed, "repeat_steps": 30,
        "python_body_calls": callback_calls[0], "parked_submission_seconds": 0.75,
        "max_error": max_error, "noise_sha256": noise_digest,
        "cancel_completed_steps": stopped.completed_steps,
        "reclaimed_after_close": released, "torch": torch.__version__,
        "runtime": importlib.metadata.version("cozy-runtime"),
        "tensorfs": importlib.metadata.version("tensorfs"),
        "native_sha256": hashlib.sha256(Path(native.__file__).read_bytes()).hexdigest(),
    })


if __name__ == "__main__":
    main()
