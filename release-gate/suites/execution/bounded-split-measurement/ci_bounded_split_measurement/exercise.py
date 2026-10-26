"""Real CUDA allocations under a256MiBprocesscap; no mocks or injected allocation errors."""

import json
import os
import sys
import time

import torch
from cozy_runtime._build_provenance import COMMIT
from cozy_runtime.author import Cut, _split, split
from cozy_runtime.internal import accel, execution, residency
from cozy_runtime.internal.execution import catch, cuts

MIB = 1 << 20


def exercise(mode):
    assert mode in ("ordinary", "pressure")
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open("cuda:0", budget_bytes=256 * MIB)
    memory.apply_budget()
    _split.install(execution)
    rows, width = 4096, 64
    x = (torch.arange(rows * width, device="cuda", dtype=torch.float32) % 256).view(
        1, rows, width
    )
    reference = x * 2 + 3
    attempted, succeeded = [], []

    def operation(value):
        count = value.shape[1]
        attempted.append(count)
        if mode == "pressure":
            # Real temporary workspace scales by token, like a declared pure FFN.
            scratch = torch.empty(count * 4 * MIB, dtype=torch.uint8, device="cuda")
        result = value * 2 + 3
        if mode == "pressure":
            del scratch
        succeeded.append(count)
        return result

    records = []
    held = None
    try:
        for turn in range(3):
            if mode == "pressure" and turn == 1:
                held = torch.empty(128 * MIB, dtype=torch.uint8, device="cuda")
            elif turn == 2:
                held = None
            if held is not None:
                assert held.numel() == 128 * MIB
            attempted.clear()
            succeeded.clear()
            with (
                memory.request(f"bounded-split-{mode}-{turn}") as request,
                catch.serving(request),
                torch.no_grad(),
            ):
                began = time.perf_counter()
                actual = split(operation, x, Cut(1))
                torch.cuda.synchronize()
                elapsed = time.perf_counter() - began
                torch.testing.assert_close(actual, reference, rtol=0, atol=0)
                events = catch.cut_rows(request)
                points = {key: list(value) for key, value in cuts._MEASURED.items()}
                assert len(points) == 1, points
                point = next(iter(points.values()))
                count = max(succeeded)
                if turn < 2:
                    assert count * width in {w for w, _ in point}, (count, point)
                records.append(
                    {
                        "turn": turn,
                        "seconds": elapsed,
                        "attempt_count": len(attempted),
                        "successful_calls": len(succeeded),
                        "largest_attempted_rows": max(attempted),
                        "largest_successful_rows": count,
                        "measured": points,
                        "cuts": [
                            {
                                "site": e.site,
                                "level": e.level,
                                "descent": e.descent,
                                "retries": e.retries,
                            }
                            for e in events.sites
                        ],
                        "exact_output": True,
                    }
                )
                del actual
        if mode == "ordinary":
            assert records[0]["largest_successful_rows"] == 128, records
            assert records[1]["largest_successful_rows"] == 256, records
            assert records[0]["successful_calls"] == 32, records
            assert records[2]["largest_successful_rows"] > 256, records
        else:
            assert records[0]["largest_attempted_rows"] == 128, records
            assert records[0]["largest_successful_rows"] < 128, records
            assert (
                records[1]["largest_successful_rows"]
                < records[0]["largest_successful_rows"]
            ), records
            final = next(iter(records[1]["measured"].values()))
            assert len(final) == 2 and final == sorted(final), final
            assert any(e["descent"] for row in records for e in row["cuts"]), records
        return {
            "pid": os.getpid(),
            "runtime_source": COMMIT,
            "device": torch.cuda.get_device_name(),
            "mode": mode,
            "local_cap_bytes": 256 * MIB,
            "records": records,
            "scope": "Real component sizing, numerical equality and allocator descent; not H3 model performance.",
        }
    finally:
        held = None
        _split.install(None)
        memory.trim()


if __name__ == "__main__":
    print(json.dumps(exercise(sys.argv[1]), sort_keys=True))
