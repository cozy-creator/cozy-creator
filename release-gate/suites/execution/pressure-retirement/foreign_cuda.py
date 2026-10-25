"""Finite controller-owned CUDA consumer, deliberately outside TensorD's process family."""

import argparse
import json
import os
import time

import torch

MIB = 1 << 20


def report(**fields):
    print(json.dumps(fields), flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--max-mib", required=True, type=int)
    args = parser.parse_args()
    if not 512 <= args.max_mib <= 16 * 1024:
        raise ValueError("explicit foreign allocation bound must be 512..16384 MiB")
    torch.set_num_threads(1)
    free, total = torch.cuda.mem_get_info()
    headroom = max(total // 10, 512 * MIB)
    target = max(headroom - 256 * MIB, 384 * MIB)
    wanted = free - target
    if not 128 * MIB <= wanted <= args.max_mib * MIB:
        raise RuntimeError(
            "explicit foreign bound cannot safely reach observed headroom"
        )
    chunks = []
    allocated = 0
    began = time.time_ns()
    try:
        while allocated < wanted:
            size = min(64 * MIB, wanted - allocated)
            free, _ = torch.cuda.mem_get_info()
            if free - size < 384 * MIB:
                raise RuntimeError(
                    "other demand changed the observed safe allocation bound"
                )
            chunk = torch.empty(size, dtype=torch.uint8, device="cuda")
            chunk.fill_(37)
            chunks.append(chunk)
            allocated += size
            torch.cuda.synchronize()
            free, _ = torch.cuda.mem_get_info()
        held = time.time_ns()
        report(
            foreign_ready=True,
            pid=os.getpid(),
            parent_pid=os.getppid(),
            allocated_bytes=allocated,
            reserved_bytes=torch.cuda.memory_reserved(),
            free_bytes=free,
            headroom_bytes=headroom,
            growth_started_ns=began,
            pressure_reached_ns=held,
        )
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            now_free, _ = torch.cuda.mem_get_info()
            if now_free >= headroom + 64 * MIB:
                # This is usable capacity for the independent application after Cozy yields.
                extra = torch.full((128 * MIB,), 17, dtype=torch.uint8, device="cuda")
                torch.cuda.synchronize()
                assert extra[:4096].sum().item() == 17 * 4096
                assert chunks[0][:4096].sum().item() == 37 * 4096
                report(
                    foreign_result=True,
                    passed=True,
                    expected_free_without_yield=target,
                    observed_free_after_yield=now_free,
                    additional_used_bytes=extra.numel(),
                    capacity_observed_ns=time.time_ns(),
                )
                del extra
                return
            time.sleep(0.05)
        raise TimeoutError(
            "Cozy did not yield usable driver capacity within 60 seconds"
        )
    finally:
        chunks.clear()
        if "chunk" in locals():
            del chunk
        torch.cuda.empty_cache()
        report(foreign_released=True, at_ns=time.time_ns())


if __name__ == "__main__":
    main()
