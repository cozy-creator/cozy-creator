import hashlib
import json
import socket
import sys
import traceback
from datetime import timedelta
from pathlib import Path

import torch
import torch.distributed as dist
import torch.multiprocessing as mp
from cozy_runtime.internal.parallel import native_ulysses as native


def original(rows, rank, heads):
    values = (
        torch.arange(rows * 3 * heads * 128, dtype=torch.float32)
        .remainder(17)
        .to(torch.float16)
    )
    return values.view(1, rows, 3, heads, 128) + rank


def worker(rank, world, port, result):
    torch.set_num_threads(1)
    try:
        dist.init_process_group(
            "gloo",
            init_method=f"tcp://127.0.0.1:{port}",
            rank=rank,
            world_size=world,
            timeout=timedelta(seconds=60),
        )
        group = dist.group.WORLD
        control = native._control(group)
        passed = []
        cases = (
            [((3, 4), 3), ((0, 7), 8)]
            if world == 2
            else [((1, 2, 0, 4), 5), ((2, 1, 3, 2), 8)]
        )
        for sizes, heads in cases:
            owned = original(sizes[rank], rank, heads)
            inputs = tuple(owned[:, :, i] for i in range(3))
            padded = heads + (-heads % world)
            for step in sorted({padded, world}, reverse=True):
                local = step // world
                rows = sizes[rank]
                total = sum(sizes)
                output = torch.empty((1, rows, padded, 128), dtype=torch.float16)
                sends = [
                    torch.empty((world * rows, 1, local, 128), dtype=torch.float16)
                    for _ in range(3)
                ]
                receives = [
                    torch.empty((total, 1, local, 128), dtype=torch.float16)
                    for _ in range(3)
                ]
                landed = [
                    torch.empty((1, total, local, 128), dtype=torch.float16)
                    for _ in range(3)
                ]
                backsend = torch.empty((total, 1, local, 128), dtype=torch.float16)
                backrecv = torch.empty(
                    (world * rows, 1, local, 128), dtype=torch.float16
                )
                metadata = control.gather(
                    control.sequence,
                    "real-host-geometry",
                    {"rows": rows, "heads": heads, "step": step},
                )
                control.sequence += 1
                assert [r["rows"] for r in metadata] == list(sizes)
                assert all(r["heads"] == heads and r["step"] == step for r in metadata)
                for start in range(0, padded, step):
                    for source, target in zip(inputs, sends):
                        native.pack_heads(source, target, start, world)
                    native._cross(
                        group, None, sends, receives, [[r] * world for r in sizes], rank
                    )
                    for i in range(3):
                        landed[i].copy_(receives[i].permute(1, 0, 2, 3))
                        complete = torch.cat(
                            [
                                original(n, p, heads)[:, :, i]
                                for p, n in enumerate(sizes)
                            ],
                            dim=1,
                        )
                        padded_input = torch.zeros(
                            (1, total, padded, 128), dtype=torch.float16
                        )
                        padded_input[:, :, :heads].copy_(complete)
                        expected = padded_input[
                            :, :, start + rank * local : start + (rank + 1) * local
                        ]
                        torch.testing.assert_close(landed[i], expected, rtol=0, atol=0)
                    computed = landed[0] + landed[1] + landed[2]
                    backsend.copy_(computed.permute(1, 0, 2, 3))
                    native._cross(
                        group,
                        None,
                        [backsend],
                        [backrecv],
                        [list(sizes) for _ in range(world)],
                        rank,
                    )
                    native.unpack_heads(backrecv, output, start, world)
                torch.testing.assert_close(
                    output[:, :, :heads],
                    inputs[0] + inputs[1] + inputs[2],
                    rtol=0,
                    atol=0,
                )
                passed.append(
                    {
                        "sizes": sizes,
                        "heads": heads,
                        "step": step,
                        "packing_and_roundtrip_bit_exact": True,
                    }
                )
        refused = False
        try:
            x = torch.ones(1, 2, 4, 128, dtype=torch.float16)
            native.exchange(x, x, x, group, backend="unsupported-on-host", options={})
        except RuntimeError as e:
            assert "metadata refused before tensor collectives" in str(e)
            assert getattr(e, "varena_no_retry", False)
            refused = True
        assert refused and not torch.cuda.is_initialized()
        assert "libcuda.so" not in Path("/proc/self/maps").read_text()
        result.put(
            {
                "rank": rank,
                "world": world,
                "passed": True,
                "cases": passed,
                "actual_CPU_admission_refused": refused,
            }
        )
    except BaseException:
        result.put(
            {
                "rank": rank,
                "world": world,
                "passed": False,
                "error": traceback.format_exc(),
            }
        )
        raise
    finally:
        if dist.is_initialized():
            dist.destroy_process_group()


if __name__ == "__main__":
    torch.set_num_threads(1)
    records = []
    for world in (2, 4):
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        ctx = mp.get_context("spawn")
        queue = ctx.Queue()
        mp.spawn(worker, args=(world, port, queue), nprocs=world, join=True)
        records.extend(queue.get(timeout=5) for _ in range(world))
    print(
        json.dumps(
            {
                "passed": all(r["passed"] for r in records),
                "torch": torch.__version__,
                "torch_commit": torch.version.git_version,
                "python": sys.executable,
                "torch_path": torch.__file__,
                "GPU_qualified": False,
                "runtime_source_sha256": hashlib.sha256(
                    Path(native.__file__).read_bytes()
                ).hexdigest(),
                "scope": "actual two/four-rank CPU Gloo transport and host Store, real tensor packing/arithmetic; no native GPU provider emulation",
                "ranks": records,
            },
            indent=2,
        )
    )
