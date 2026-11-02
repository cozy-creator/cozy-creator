"""Coordinator-only packed-host GPU proof; run with the matching built TensorFS wheel.

Uses the existing public typed-export, native retirement and Varena graph harness.
No rental, download, or installation. Compilation alone is not GPU qualification.
"""

from __future__ import annotations

import argparse
import ctypes as C
import gc
import json
import time

import torch
from tensorfs.varena import Source, api
from tensorfs.varena import torch as vt


def run(device: int):
    vt.install(device)
    stream = torch.cuda.Stream(device=device)
    gpu = api.open(device)
    checks = []
    quarantine = api.retirement_stats()["quarantined"]

    def settle():
        deadline = time.monotonic() + 30
        while True:
            stats = api.retirement_stats()
            assert stats["quarantined"] == quarantine, stats
            if not stats["queued"]:
                return
            assert time.monotonic() < deadline, stats
            time.sleep(0.005)

    def export(region, source, offset):
        return region.tensor(dtype=source.dtype, shape=source.shape, strides=source.strides,
                             byte_offset=offset + source.byte_offset, stream=stream.cuda_stream)

    try:
        raw = bytearray([0x17]) * ((2 << 20) + 17)
        matrix = torch.arange(24, dtype=torch.float16).reshape(4, 6)
        view = matrix[1:, 1:5].T
        sources = (
            Source.from_buffer(raw, dtype="uint8", shape=(len(raw),)),
            Source.from_tensor(torch.arange(65, dtype=torch.float32)),
            Source.from_buffer(b"", dtype="uint8", shape=(0,)),
            Source.from_tensor(view),
        )
        expected_view = view.clone()
        region, offsets = gpu.region_parts(sources)
        assert offsets == (0, (2 << 20) + 256, (2 << 20) + 768, (2 << 20) + 768)
        expected = bytearray(offsets[-1] + sources[-1].nbytes)
        for offset, source in zip(offsets, sources):
            expected[offset:offset + source.nbytes] = source.data
        expected = torch.frombuffer(expected, dtype=torch.uint8).clone()
        raw[:] = bytes(len(raw))
        matrix.fill_(-1)
        pointer = None
        for _ in range(2):
            whole = region.tensor(dtype="uint8", shape=(expected.numel(),), stream=stream.cuda_stream)
            pointer = whole.data_ptr() if pointer is None else pointer
            assert whole.data_ptr() == pointer
            with torch.cuda.stream(stream):
                copied = whole.clone()
            stream.synchronize()
            assert torch.equal(copied.cpu(), expected)
            del whole, copied
            gc.collect()
            settle()
            gpu.trim()
            assert gpu.stats()["weights_resident"] == 0
        checks.append("mixed parts, zero gaps, stage crossing, immutable snapshot, fixed-address refill")

        old = export(region, sources[-1], offsets[-1])
        escaped = old[1:, ::2].detach()
        old_pointer = old.data_ptr()
        converted = sources[-1].derive("float32")
        newer, newer_offsets = gpu.region_parts((converted,))
        new = export(newer, converted, newer_offsets[0])
        with torch.cuda.stream(stream):
            old_copy, new_copy = old.clone(), new.clone()
            result = torch.empty_like(old)
            result.copy_(old)  # Warm the exact graph operation.
        stream.synchronize()
        settle()
        assert torch.equal(old_copy.cpu(), expected_view)
        assert torch.equal(new_copy.cpu(), expected_view.float())
        graph = torch.cuda.CUDAGraph()
        with vt.graph(graph, stream=stream):
            try:
                gpu.region_parts(sources)
            except api.Capturing:
                pass
            else:
                raise AssertionError("packed factory accepted capture")
            # Invalid readable address proves capture refusal happens before payload copy.
            bad = (api._HostPart * 1)(api._HostPart(C.c_void_p(1), 1))
            unchanged_offsets, unchanged_handle = (api._u * 1)(123), api._u(456)
            status = api._lib.vr_v1_region_from_host_parts(
                gpu._handle, bad, 1, unchanged_offsets, C.byref(unchanged_handle))
            assert status == 2 and unchanged_offsets[0] == 123 and unchanged_handle.value == 456
            result.copy_(old)
        region.close()
        newer.close()
        del region, newer, new, old_copy, new_copy
        gc.collect()
        gpu.trim()
        assert old.data_ptr() == old_pointer and gpu.stats()["weights_resident"] > 0
        for _ in range(2):
            vt.replay(graph, stream=stream)
            stream.synchronize()
            assert torch.equal(result.cpu(), expected_view)
        with torch.cuda.stream(stream):
            escaped_copy = escaped.clone()
        stream.synchronize()
        assert torch.equal(escaped_copy.cpu(), expected_view[1:, ::2])
        checks.append("capture refuses before copying, warm replay, old dtype and escaped typed aliases")
        graph.reset()
        del graph, result, old, escaped, escaped_copy
        gc.collect()
        settle()
        gpu.trim()
        assert gpu.stats()["weights_resident"] == 0
        checks.append("final typed alias release and native retirement reclaim packed Region")
        return {"checks": checks, "device": device, "torch": str(torch.__version__),
                "native_stats": gpu.stats()}
    finally:
        stream.synchronize()
        vt.retire_stream(stream, device)
        gpu.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", type=int, default=0)
    print(json.dumps(run(parser.parse_args().device)))
