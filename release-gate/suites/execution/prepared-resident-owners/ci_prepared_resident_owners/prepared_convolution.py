"""Real telemetry watcher versus PreparedRepeat capture, submission and retirement."""

import ctypes
import importlib.metadata
import json
import os
import threading
import time

import torch
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.internal import accel


def prepared_convolution_proof(seed, ctx, tel):
    assert ctx.device.type == "cuda"
    accel.initialize(torch, "cuda")
    device = torch.cuda.current_device()
    driver = ctypes.CDLL("libcuda.so.1")
    query = driver.cuStreamIsCapturing
    query.argtypes = (ctypes.c_void_p, ctypes.POINTER(ctypes.c_int))
    query.restype = ctypes.c_int
    status_value = ctypes.c_int(-1)
    status_pointer = ctypes.pointer(status_value)
    report = {
        "seed": seed,
        "mode": "conv-prepared-watched",
        "request_id": ctx.request_id,
        "pid": os.getpid(),
        "runtime": importlib.metadata.version("cozy-runtime"),
        "tensorfs": importlib.metadata.version("tensorfs"),
        "torch": str(torch.__version__),
        "cudnn": torch.backends.cudnn.version(),
        "device": torch.cuda.get_device_name(device),
        "capture_status": [],
        "body_calls": 0,
        "capture_pauses": 0,
    }

    def values(shape, divisor, modulus):
        size = 1
        for length in shape:
            size *= length
        return (
            ((torch.arange(size) + seed % modulus) % modulus - modulus // 2)
            .float()
            .div(divisor)
            .to(dtype=torch.float16, device=f"cuda:{device}")
            .reshape(shape)
        )

    def status(stage):
        code = query(stream_pointer, status_pointer)
        value = status_value.value if code == 0 else None
        report["capture_status"].append({"stage": stage, "code": code, "status": value})
        assert (code, value) == (0, 1), (stage, code, value)

    def samples():
        # Observe the real watch only outside capture: the production pause owns this lock.
        with watch.lock:
            return sum(watch.counts.values())

    def annotate(error):
        error.add_note(
            "[prepared-watched-convolution] "
            + json.dumps(report, separators=(",", ":"))
        )

    plan = None
    with torch.inference_mode(False), torch.no_grad():
        x = values((2, 4, 128, 128), 64, 31)
        weight = values((320, 4, 3, 3), 64, 17)
        bias = values((320,), 1024, 13)

        def convolution():
            return torch.ops.aten.convolution.default(
                x, weight, bias, [1, 1], [1, 1], [1, 1], False, [0, 0], 1
            )

        on_step = tel.step_callback(2, stage="convolution")
        watch = tel._attempt._stall_watch
        assert watch is not None, "real CUDA telemetry watch was not created"
        report["watcher_threads"] = [
            t.ident for t in threading.enumerate() if t.name == "cozy-host-stalls"
        ]
        assert report["watcher_threads"], "real stall sampler did not start"
        eager = convolution()
        on_step(0)
        state = torch.zeros_like(eager)
        report["samples_before_prepare"] = samples()
        body_stream = None
        stream_pointer = None

        def body(index):
            nonlocal body_stream, stream_pointer
            del index
            report["body_calls"] += 1
            capturing = torch.cuda.is_current_stream_capturing()
            if body_stream is None:
                assert not capturing, "PreparedRepeat must warm up before capture"
                body_stream = torch.cuda.current_stream(device)
                stream_pointer = ctypes.c_void_p(body_stream.cuda_stream)
            if capturing:
                report["capture_pauses"] += 1
                status("before-pause")
                time.sleep(
                    0.3
                )  # Host pause only while the real body is being captured.
                status("after-pause")
            state.copy_(convolution())

        try:
            plan = PreparedRepeat.capture(body, state=(state,), steps=2)
            assert report["body_calls"] == 2 and report["capture_pauses"] == 1, report
            report["samples_after_prepare"] = samples()
            # The sampler must resume after the capture lock is released. No callback or
            # idle predicate is replaced; these are the service's own successful samples.
            deadline = time.monotonic() + 1.0
            while samples() <= report["samples_after_prepare"]:
                ctx.raise_if_cancelled()
                assert time.monotonic() < deadline, (
                    "real watcher did not resume after capture"
                )
                time.sleep(0.02)
            report["samples_after_resume"] = samples()
            execution = ctx.plan(plan)  # Exactly one submitted plan for this request.
            output = execution.wait()[0]
            progress = execution.poll()
            assert (
                progress.done
                and not progress.canceled
                and progress.completed_steps == 2
            )
            report["completed_steps"] = progress.completed_steps
            torch.testing.assert_close(output, eager, rtol=1e-3, atol=1e-3)
            report["max_abs"] = float((output.float() - eager.float()).abs().max())
            report["native"] = plan.facts()
            assert report["native"]["host_graph_launches"] == 1, report
            assert report["body_calls"] == 2, "GPU replay called the Python body"
            on_step(1)
            assert watch.done.is_set(), "normal final step did not stop the watcher"
        except BaseException as error:
            annotate(error)
            raise
        finally:
            if plan is not None:
                try:
                    plan.close()
                except BaseException as error:
                    annotate(error)
                    raise
    return report
