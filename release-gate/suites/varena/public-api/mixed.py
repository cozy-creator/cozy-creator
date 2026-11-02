"""Real mixed-binding client. Default is host-only; --gpu requires owner authorization."""

from __future__ import annotations

import argparse
import ctypes as C
import json
import threading
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--c", type=Path, required=True)
    parser.add_argument("--rust", type=Path, required=True)
    parser.add_argument(
        "--order", choices=["native-first", "python-first"], required=True
    )
    parser.add_argument("--gpu", action="store_true")
    args = parser.parse_args()
    if args.order == "native-first":
        c = C.CDLL(str(args.c.resolve()), mode=C.RTLD_GLOBAL)
        rust = C.CDLL(str(args.rust.resolve()), mode=C.RTLD_GLOBAL)
    import tensorfs.varena.torch as vt
    from tensorfs import _ext
    from tensorfs.varena import api

    if args.order == "python-first":
        c = C.CDLL(str(args.c.resolve()), mode=C.RTLD_GLOBAL)
        rust = C.CDLL(str(args.rust.resolve()), mode=C.RTLD_GLOBAL)
    for function in [
        c.mixed_c_identity,
        rust.mixed_rust_identity,
        rust.mixed_internal_identity,
        rust.mixed_internal_sequence,
    ]:
        function.restype = C.c_uint64
    identities = [
        c.mixed_c_identity(),
        rust.mixed_rust_identity(),
        rust.mixed_internal_identity(),
        _ext._varena_core_identity(),
        api.core_identity(),
    ]
    assert len(set(identities)) == 1 and identities[0] != 0, identities
    assert c.mixed_c_error() == 1
    assert c.mixed_c_contract() == 1
    assert c.mixed_c_error() == 1
    # The C client set the actual core's thread-local error. Python reads that same
    # TLS slot; calling an independent copy could not observe this failure.
    assert api._lib.vr_v1_last_error() == b"device must be nonnegative"
    assert rust.mixed_rust_error() == 1
    assert api._lib.vr_v1_last_error() == b"device must be nonnegative"
    # Invalid handles, including a double close, are errors rather than C pointer UB.
    assert api._lib.vr_v1_gpu_close(2**64 - 1) == 5
    main_error = api._lib.vr_v1_last_error()
    errors = []

    def other_thread():
        assert c.mixed_c_error() == 1
        errors.append(api._lib.vr_v1_last_error())

    thread = threading.Thread(target=other_thread)
    thread.start()
    thread.join()
    assert (
        errors == [b"device must be nonnegative"]
        and api._lib.vr_v1_last_error() == main_error
    )
    mapped = {
        line.split()[-1]
        for line in Path("/proc/self/maps").read_text().splitlines()
        if line.endswith("/libvarena.so")
    }
    assert len(mapped) == 1, mapped
    # The actual internal owner registry's monotone request sequence is live too.
    sequence = [rust.mixed_internal_sequence(), rust.mixed_internal_sequence()]
    assert sequence[1] == sequence[0] + 1
    result = {
        "scope": "host",
        "order": args.order,
        "identities": identities,
        "mapped_core": sorted(mapped),
        "shared_tls": True,
        "thread_local_errors": True,
        "native_request_sequence": sequence,
        "gpu_executed": False,
    }
    if args.gpu:
        vt.install(0)
        import torch

        # Include the first real Torch dispatch's persistent library ownership in
        # the baseline, rather than misclassifying initialization as a binding leak.
        warm = torch.arange(1024, device="cuda", dtype=torch.int32)
        assert warm.sum().item() == 1023 * 1024 // 2
        del warm
        torch.cuda.current_stream().synchronize()
        from streams import owned_stream

        with owned_stream(torch, vt) as stream, torch.cuda.stream(stream), api.open() as gpu:
            gpu.trim()
            before = gpu.stats()
            allocation = C.c_uint64()
            c.mixed_c_alloc.argtypes = [C.c_uint64, C.c_uint64, C.c_uint64, C.POINTER(C.c_uint64)]
            rust.mixed_rust_gpu.argtypes = [C.c_uint64]
            c.mixed_c_free.argtypes = [C.c_uint64]
            assert c.mixed_c_alloc(gpu._handle, 2 << 20, stream.cuda_stream, C.byref(allocation)) == 0
            after_c = gpu.stats()
            assert after_c["working_live"] - before["working_live"] == 2 << 20
            native = _ext._varena_working_stats(0)
            assert native["allocated_bytes"] == after_c["working_live"], native
            value = torch.arange(1024, device="cuda", dtype=torch.int32)
            assert value.sum().item() == 1023 * 1024 // 2
            after_torch = gpu.stats()
            assert after_torch["working_live"] > after_c["working_live"]
            assert rust.mixed_rust_gpu(stream.cuda_stream) == 1
            assert c.mixed_c_free(allocation.value) == 0
            del value
            stream.synchronize()
            gpu.trim()
            final = gpu.stats()
            assert final["working_live"] == before["working_live"], (before, final)
            assert final["working_mapped"] <= before["working_mapped"], (before, final)
            result.update(
                scope="gpu",
                gpu_executed=True,
                before=before,
                after_c=after_c,
                after_torch=after_torch,
                final=final,
            )
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
