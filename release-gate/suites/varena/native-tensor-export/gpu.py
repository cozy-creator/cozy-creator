"""Coordinator-only real CUDA client for public typed tensor exports and Sources.

Run inside an ordinary Cozy release-gate invocation with the matching native core,
TensorFS extension and Torch shim. This client rents no machine and downloads no
weights. Host compilation/import checks do not qualify any of these GPU claims.
"""

from __future__ import annotations

import argparse
import gc
import json
import time

import torch
from tensorfs.varena import Source, api
from tensorfs.varena import torch as vt


def descriptor(source):
    return {
        "dtype": source.dtype,
        "shape": source.shape,
        "strides": source.strides,
        "byte_offset": source.byte_offset,
    }


def run(device: int, delay_cycles: int):
    # Installation precedes Torch CUDA initialization. These retained Torch
    # streams are nonblocking and exist before any work in the client is queued.
    vt.install(device)
    first = torch.cuda.Stream(device=device)
    second = torch.cuda.Stream(device=device)
    side = torch.cuda.Stream(device=device)
    streams = (first, second, side)
    gpu = api.open(device)
    checks = []
    quarantine_baseline = api.retirement_stats()["quarantined"]

    def settle_native():
        # Wait on checked native retirement observations, not an assumed CUDA
        # delay. The coordinator owns cancellation/budget for this proof client.
        while True:
            stats = api.retirement_stats()
            assert stats["quarantined"] == quarantine_baseline, stats
            if not stats["queued"]:
                return
            time.sleep(0.005)

    try:
        original = torch.arange(512 * 512, dtype=torch.float32).reshape(512, 512)
        original.div_(1024)
        host_view = original[1:500:2, 3:500:3].T
        expected = host_view.clone()
        source = Source.from_tensor(host_view)
        original.fill_(-77)
        region = gpu.region(source)
        used = gpu.use([region], first.cuda_stream)
        first_export = used.tensor(**descriptor(source))
        # This is a distinct foreign storage owner, not a view of first_export.
        # No first-stream wait is installed on second: native export ordering
        # must order the actual in-flight Source producer on the override stream.
        second_export = used.tensor(**descriptor(source), stream=second.cuda_stream)
        assert first_export.data_ptr() == second_export.data_ptr()
        assert first_export.dtype == torch.float32
        assert first_export.shape == expected.shape
        assert first_export.stride() == source.strides
        with torch.cuda.stream(first):
            copied_first = first_export.clone()
        with torch.cuda.stream(second):
            copied_second = second_export.clone()
        first.synchronize()
        second.synchronize()
        assert torch.equal(copied_first.cpu(), expected)
        assert torch.equal(copied_second.cpu(), expected)
        settle_native()
        checks.append(
            "typed interior export and independent override-stream producer ordering"
        )

        escaped = first_export[1:, ::2].detach().T
        escaped_expected = expected[1:, ::2].T.contiguous()
        assert escaped.data_ptr() > first_export.data_ptr()
        # The temporary public handles end while two independent storage Pins
        # remain: escaped retains first_export's storage, second_export its own.
        used.close()
        region.close()
        del used, region, first_export, copied_first, copied_second
        gc.collect()
        pinned = gpu.stats()["weights_resident"]
        assert pinned > 0
        gpu.trim()
        assert gpu.stats()["weights_resident"] == pinned
        checks.append(
            "Use/Region close and trim preserve escaped and independent storage Pins"
        )

        with torch.cuda.stream(side):
            side.wait_stream(first)
            side.wait_stream(second)
            # Both exported DataPtrs are interior pointers into the native
            # reservation. The allocator hook must route them to their native
            # consumer families, including the detached/transposed view.
            escaped.record_stream(side)
            second_export.record_stream(side)
            torch.cuda._sleep(delay_cycles)
            copied_escaped = escaped.clone()
            copied_independent = second_export.clone()
            done = torch.cuda.Event()
            done.record(side)
        assert not done.query(), (
            "side work already completed; rerun with a larger --delay-cycles "
            "to qualify final-alias release while consumers are pending"
        )
        del escaped, second_export
        gc.collect()
        gpu.trim()
        assert done.query(), "trim returned before the recorded last consumer completed"
        assert gpu.stats()["weights_resident"] == 0
        side.synchronize()
        assert torch.equal(copied_escaped.cpu(), escaped_expected)
        assert torch.equal(copied_independent.cpu(), expected)
        checks.append(
            "interior record_stream and final-alias release wait actual pending consumers"
        )
        del copied_escaped, copied_independent, done

        old_bytes = source.data
        half_source = source.derive(torch.float16)
        restored_source = half_source.derive(torch.float32)
        assert source.data == old_bytes
        half_region = gpu.region(half_source)
        restored_region = gpu.region(restored_source)
        half_export = half_region.tensor(
            **descriptor(half_source), stream=first.cuda_stream
        )
        restored_export = restored_region.tensor(
            **descriptor(restored_source), stream=first.cuda_stream
        )
        half_region.close()
        restored_region.close()
        with torch.cuda.stream(first):
            copied_half = half_export.clone()
            copied_restored = restored_export.clone()
        del half_region, restored_region, half_export, restored_export
        gc.collect()
        first.synchronize()
        settle_native()
        assert torch.equal(copied_half.cpu(), expected.half())
        assert torch.equal(copied_restored.cpu(), expected.half().float())
        assert half_source.strides == source.strides
        assert half_source.byte_offset * 2 == source.byte_offset
        checks.append("immutable host snapshot and half/float Source storage versions")
        del copied_half, copied_restored

        # Real native layout rejections; the public Use remains usable afterward.
        with (
            gpu.region(bytes(16)) as small,
            gpu.use([small], first.cuda_stream) as used,
        ):
            refused = 0
            for metadata in (
                {"dtype": "float32", "shape": (5,)},
                {"dtype": "float32", "shape": (1,), "byte_offset": 1},
                {"dtype": "float32", "shape": (2,), "strides": (-1,)},
                {"dtype": "float32", "shape": (2,), "strides": ()},
                {"dtype": "float32", "shape": (2,), "strides": (4,)},
            ):
                try:
                    used.tensor(**metadata)
                except (ValueError, OverflowError):
                    refused += 1
                else:
                    raise AssertionError(f"invalid native export accepted: {metadata}")
            assert refused == 5
            scalar = used.tensor(dtype="uint8", shape=(), byte_offset=3)
            broadcast = used.tensor(dtype="uint8", shape=(2, 3), strides=(0, 0))
            empty = used.tensor(dtype="uint8", shape=(0, 3), byte_offset=16)
            assert scalar.shape == () and broadcast.stride() == (0, 0)
            assert empty.numel() == 0
            with torch.cuda.stream(first):
                scalar_value = scalar.clone()
                broadcast_value = broadcast.clone()
            del scalar, broadcast, empty
        first.synchronize()
        settle_native()
        assert scalar_value.cpu().item() == 0
        assert torch.equal(
            broadcast_value.cpu(), torch.zeros((2, 3), dtype=torch.uint8)
        )
        del scalar_value, broadcast_value
        gc.collect()
        gpu.trim()
        assert gpu.stats()["weights_resident"] == 0
        checks.append(
            "native dtype/layout bounds rejection and scalar/empty/broadcast exports"
        )
        return {
            "torch": str(torch.__version__),
            "cuda": torch.version.cuda,
            "device": device,
            "name": torch.cuda.get_device_name(device),
            "delay_cycles": delay_cycles,
            "checks": checks,
            "native_stats": gpu.stats(),
        }
    finally:
        for stream in streams:
            stream.synchronize()
        # Every consumer storage has ended before successful stream retirement.
        # Torch retains ownership of its stream-pool handles; no raw destruction.
        for stream in streams:
            vt.retire_stream(stream, device)
        gpu.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", type=int, default=0)
    parser.add_argument("--delay-cycles", type=int, default=50_000_000)
    arguments = parser.parse_args()
    if arguments.delay_cycles <= 0:
        parser.error("--delay-cycles must be positive")
    print(json.dumps(run(arguments.device, arguments.delay_cycles)))
