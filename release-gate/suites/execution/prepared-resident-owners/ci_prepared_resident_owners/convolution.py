"""The first full-SDXL convolution, without a checkpoint or model download."""

import ctypes
import importlib.metadata
import json
import os
import threading
import time

import torch
from cozy_runtime.internal import accel
from tensorfs import varena


def convolution_proof(seed, ctx, backing, *, telemetry=None):
    assert ctx.device.type == "cuda"
    assert backing in ("torch", "varena")
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
        "mode": "conv-watched" if telemetry is not None else f"conv-{backing}",
        "request_id": ctx.request_id,
        "pid": os.getpid(),
        "runtime": importlib.metadata.version("cozy-runtime"),
        "tensorfs": importlib.metadata.version("tensorfs"),
        "torch": str(torch.__version__),
        "cudnn": torch.backends.cudnn.version(),
        "device": torch.cuda.get_device_name(device),
        "benchmark": torch.backends.cudnn.benchmark,
        "deterministic": torch.backends.cudnn.deterministic,
        "allow_tf32": torch.backends.cudnn.allow_tf32,
        "memory": [],
        "status": [],
        "failures": [],
    }
    arena = varena.Arena(device) if backing == "varena" else None
    held = []

    def values(shape, divisor, modulus):
        size = 1
        for length in shape:
            size *= length
        return (
            ((torch.arange(size) + seed % modulus) % modulus - modulus // 2)
            .float()
            .div(divisor)
            .to(torch.float16)
            .reshape(shape)
        )

    def weight(source):
        if arena is None:
            return source.to(device=f"cuda:{device}")
        region = arena.reserve(source.numel() * source.element_size())
        use = region.prepare()
        assert use.needs_fill
        tensor = use.tensor.view(torch.float16).reshape(source.shape)
        tensor.copy_(source)
        use.commit()
        held.append((region, use, tensor))
        return tensor

    def memory(stage):
        free, total = torch.cuda.mem_get_info(device)
        report["memory"].append(
            {
                "stage": stage,
                "free": free,
                "total": total,
                "allocated": torch.cuda.memory_allocated(device),
                "reserved": torch.cuda.memory_reserved(device),
            }
        )

    def capture_status(stage):
        code = query(stream_pointer, status_pointer)
        value = status_value.value if code == 0 else None
        report["status"].append({"stage": stage, "code": code, "status": value})
        assert (code, value) == (0, 1), (stage, code, value)

    def note(error, stage):
        report["failures"].append(
            {"stage": stage, "type": type(error).__name__, "error": str(error)[:600]}
        )
        error.add_note(
            "[minimal-convolution] " + json.dumps(report, separators=(",", ":"))
        )

    with torch.inference_mode(False), torch.no_grad():
        x = values((2, 4, 128, 128), 64, 31).to(device=f"cuda:{device}")
        w = weight(values((320, 4, 3, 3), 64, 17))
        bias = weight(values((320,), 1024, 13))
        report["tensors"] = [
            {"shape": list(t.shape), "stride": list(t.stride()), "dtype": str(t.dtype)}
            for t in (x, w, bias)
        ]
        report["convolution"] = {
            "stride": [1, 1],
            "padding": [1, 1],
            "dilation": [1, 1],
            "transposed": False,
            "output_padding": [0, 0],
            "groups": 1,
        }

        def convolution():
            return torch.ops.aten.convolution.default(
                x, w, bias, [1, 1], [1, 1], [1, 1], False, [0, 0], 1
            )

        # Match the failed path: eager first, a fresh stream's warmup, then manual
        # global capture. Do not add empty_cache/device-wide synchronization here.
        on_step = None
        watch = None
        if telemetry is not None:
            on_step = telemetry.step_callback(2, stage="convolution")
            # Observe the real service-created watch; do not replace its idle callback.
            watch = telemetry._attempt._stall_watch
            assert watch is not None, "real CUDA telemetry watch was not created"
            report["watcher"] = {
                "workers": [
                    t.ident
                    for t in threading.enumerate()
                    if t.name == "cozy-host-stalls"
                ],
                "pause_seconds": 0.3,
            }
            assert report["watcher"]["workers"], "real stall sampler did not start"
        eager = convolution()
        if on_step is not None:
            on_step(0)
        current = torch.cuda.current_stream(device)
        stream = torch.cuda.Stream(device=device)
        stream_pointer = ctypes.c_void_p(stream.cuda_stream)
        stream.wait_stream(current)
        graph = torch.cuda.CUDAGraph(keep_graph=True)
        stage = "before-warmup"
        try:
            memory(stage)
            with torch.cuda.stream(stream):
                warm = convolution()
            stream.synchronize()
            del warm
            memory("after-warmup-before-capture")
            with varena.capture(device), torch.cuda.stream(stream):
                stage = "capture-begin"
                graph.capture_begin(capture_error_mode="global")
                try:
                    if watch is not None:
                        stage = "watcher-pause"
                        capture_status("before-watcher-pause")
                        with watch.lock:
                            report["watcher"]["samples_before"] = sum(
                                watch.counts.values()
                            )
                        time.sleep(
                            0.3
                        )  # Real 100ms sampler runs while global capture is open.
                        with watch.lock:
                            report["watcher"]["samples_after"] = sum(
                                watch.counts.values()
                            )
                        report["watcher"]["workers_after"] = [
                            t.ident
                            for t in threading.enumerate()
                            if t.name == "cozy-host-stalls"
                        ]
                        capture_status("after-watcher-pause")
                    stage = "convolution"
                    capture_status("before-convolution")
                    captured = convolution()
                    capture_status("after-convolution")
                except BaseException as error:
                    note(error, stage)
                    raise
                finally:
                    try:
                        graph.capture_end()
                    except BaseException as error:
                        note(error, "capture-end")
                        raise
            stream.synchronize()
            stage = "instantiate-replay"
            with torch.cuda.stream(stream):
                graph.instantiate()
                graph.replay()
            stream.synchronize()
            stage = "compare"
            torch.testing.assert_close(captured, eager, rtol=1e-3, atol=1e-3)
            report["max_abs"] = float((captured.float() - eager.float()).abs().max())
            report["manual_graph_launches"] = 1
            if on_step is not None:
                on_step(1)  # Normal service completion stops its own watcher.
            if arena is not None:
                report["varena"] = arena.stats()
        except BaseException as error:
            note(error, stage)
            raise
        finally:
            try:
                graph.reset()
            except BaseException as error:
                note(error, "graph-reset")
                raise
    return report
