"""cr-348: what a 2-GPU group's crossings cost on this pod, through the Runtime's own route.

Run with an executor environment's Python (its torch, Diffusers, Runtime and TensorFS). The parent measures
the pair (`links`, as a machine does at boot), then starts one process per GPU. They form an NCCL group the
way an executor's group forms (`group`), map the exchange the links name with a varena host tier's inbox
(`copy_engine.form`), and time, at LTX-2.5 1080p's shapes (5 s: 8,160 video and 126 audio tokens at the
base, 32,640 at the refine):
- raw: the exchange and NCCL's all_to_all_single moving the same bytes, 1-256 MiB a peer;
- self: `ulysses.exchange` around SDPA (bf16 crossing), against SDPA alone on this GPU's share;
- cross: ltx_2_5/parallel.py's cross-modal pattern (Diffusers' `all_to_all_single_any_*_async`: q, k, v,
  then o), against the same bytes in one exchange crossing each way;
- sync: whether a call returns to the host before the GPU's queue drains (20 ms of matmuls queued first);
- meet: the per-call rendezvous (`ulysses._Rendezvous.gather`) in lockstep and with one GPU 2 ms late.
One JSON object a line on stdout, each GPU's.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time

WORLD = 2


def main() -> int:
    if sys.argv[1:2] == ["--rank"]:
        return rank(int(sys.argv[2]), sys.argv[3])
    from cozy_runtime.internal.parallel import links

    devices = [str(i) for i in range(WORLD)]
    found = links.measured(None, devices)
    print(json.dumps({"links": found.document(devices), "exchange": found.exchange, "why": found.why()}), flush=True)
    store = os.path.join(tempfile.mkdtemp(prefix="cr348-"), "store")
    ranks = [
        subprocess.Popen([sys.executable, os.path.abspath(__file__), "--rank", str(r), store], stdout=subprocess.PIPE, text=True)
        for r in range(WORLD)
    ]
    outs = [p.communicate()[0] for p in ranks]
    for out in outs:
        sys.stdout.write(out)
    return max(p.returncode for p in ranks)


def rank(r: int, store_path: str) -> int:
    import torch
    import torch.distributed as dist
    import torch.nn.functional as F

    from cozy_runtime.internal.parallel import copy_engine, group, ulysses

    def emit(what: str, **row: object) -> None:
        print(json.dumps({"gpu": r, "what": what, **row}), flush=True)

    os.environ[group.NVLS_ENV] = "0"
    group.abortable("nccl")
    group.route({"NCCL_P2P_LEVEL": "NVL"})
    torch.cuda.set_device(r)
    device = torch.device("cuda", r)
    dist.init_process_group(
        "nccl", store=group.rendezvous(torch, store_path, WORLD), rank=r, world_size=WORLD, device_id=device, timeout=group.NEVER
    )
    group.warm(torch, "nccl", r, WORLD)
    pg = dist.group.WORLD
    from tensorfs.varena._runtime import Pages

    pages = Pages(r, cap=8 << 30)
    exchange, check = copy_engine.form(torch, pg, r, WORLD, "host-staged", pages)
    copy_engine.install(exchange, pg)
    emit("formed", check=check, exchange=None if exchange is None else exchange.name, torch=torch.__version__,
         runtime=__import__("cozy_runtime").__file__, name=torch.cuda.get_device_name(device))
    if exchange is None:
        return 1
    import inspect

    # cr-348's exchange takes a matrix a tensor and crosses a cross-attention (Q rows unlike K/V's).
    new = "matrices" in inspect.signature(exchange.start).parameters

    def a2a(sends, matrix, out) -> None:
        exchange.all_to_all_into(sends, [matrix] * len(sends) if new else matrix, out)

    def timed(fn, n: int, warm: int = 3) -> dict[str, float]:
        for _ in range(warm):
            fn()
        torch.cuda.synchronize()
        dist.barrier()
        begin, end = torch.cuda.Event(enable_timing=True), torch.cuda.Event(enable_timing=True)
        t0 = time.perf_counter()
        begin.record()
        for _ in range(n):
            fn()
        host = time.perf_counter() - t0
        end.record()
        torch.cuda.synchronize()
        wall = time.perf_counter() - t0
        return {"ms": round(wall / n * 1e3, 3), "host_ms": round(host / n * 1e3, 3), "gpu_ms": round(begin.elapsed_time(end) / n, 3)}

    # raw: one tensor of 1 KiB rows, half to each GPU.
    for mib in (1, 4, 16, 64, 256):
        rows = (mib << 20) // 1024
        send = torch.randn(WORLD * rows, 512, dtype=torch.bfloat16, device=device)
        out = torch.empty_like(send)
        matrix = [[rows] * WORLD for _ in range(WORLD)]
        n = max(4, min(50, 2048 // mib))
        staged = timed(lambda: a2a([send], matrix, [out]), n)
        nccl = timed(lambda: dist.all_to_all_single(out, send, group=pg), n)
        peer = rows * 1024
        emit("raw", mib_to_peer=mib, staged=staged, staged_gbps=round(peer / staged["ms"] / 1e6, 2),
             nccl=nccl, nccl_gbps=round(peer / nccl["ms"] / 1e6, 2))
        del send, out

    def sdpa(q, k, v):
        return F.scaled_dot_product_attention(q.transpose(1, 2), k.transpose(1, 2), v.transpose(1, 2)).transpose(1, 2)

    a = torch.randn(4096, 4096, dtype=torch.bfloat16, device=device)
    t = timed(lambda: a @ a, 20)
    busy_n = max(1, round(20 / t["gpu_ms"]))

    def busy() -> None:
        for _ in range(busy_n):
            a @ a

    def returns(fn) -> dict[str, float]:
        """Host ms until `fn` returns with 20 ms of matmuls queued before it, against those alone."""
        torch.cuda.synchronize()
        dist.barrier()
        t0 = time.perf_counter()
        busy()
        fn()
        back = time.perf_counter() - t0
        torch.cuda.synchronize()
        done = time.perf_counter() - t0
        return {"returned_ms": round(back * 1e3, 2), "done_ms": round(done * 1e3, 2), "queued_ms": round(busy_n * t["gpu_ms"], 2)}

    from diffusers.models.attention_dispatch import all_to_all_single_any_o_async, all_to_all_single_any_qkv_async

    audio = 126
    for stage, video in (("base", 8160), ("refine", 32640)):
        mine = video // WORLD
        # self: the video self-attention, 32 heads x 128, through the Runtime's Ulysses exchange.
        q, k, v = (torch.randn(1, mine, 32, 128, dtype=torch.bfloat16, device=device) for _ in range(3))
        n = 20 if stage == "base" else 6
        ulysses.taken()
        through = timed(lambda: ulysses.exchange(q, k, v, pg, sdpa, "bench self"), n)
        clock = ulysses.taken()
        whole = torch.randn(1, video, 16, 128, dtype=torch.bfloat16, device=device)
        alone = timed(lambda: sdpa(whole, whole, whole), n)
        mine_out = ulysses.exchange(q, k, v, pg, sdpa, "bench self")
        full = [torch.empty((WORLD, *x.shape), dtype=x.dtype, device=device) for x in (q, k, v)]
        for x, into in zip((q, k, v), full):
            dist.all_gather_into_tensor(into, x)
        ref = sdpa(*(x.transpose(0, 1).flatten(1, 2) for x in full))[:, r * mine : (r + 1) * mine]
        emit("self", stage=stage, rows=mine, exchange=through, sdpa_share=alone, clock=clock,
             exact=bool(torch.equal(mine_out, ref)), max_diff=float((mine_out.float() - ref.float()).abs().max()),
             sync=returns(lambda: ulysses.exchange(q, k, v, pg, sdpa, "bench self")))
        del full, ref, mine_out
        del q, k, v, whole
        # cross: a2v (q video, kv audio) and v2a (q audio, kv video), 32 heads x 64, as ltx_2_5/parallel.py crosses.
        vq = torch.randn(1, mine, 32, 64, dtype=torch.bfloat16, device=device)
        aq = torch.randn(1, audio // WORLD, 32, 64, dtype=torch.bfloat16, device=device)

        def package(query, key, value):
            heads, rows = query.shape[2], query.shape[1]
            waits = [all_to_all_single_any_qkv_async(x, pg) for x in (query, key, value)]
            qq, kk, vv = (w() for w in waits)
            o = sdpa(qq, kk, vv)
            return all_to_all_single_any_o_async(o, pg, NUM_QO_HEAD=heads, Q_S_LOCAL=rows)()

        a2v = timed(lambda: package(vq, aq, aq), n)
        v2a = timed(lambda: package(aq, vq, vq), n)
        # The same bytes in one crossing each way: a2v's q out and o back; v2a's k and v out (a2v's audio is noise).
        half = mine * 16 * 64  # elements of a peer's heads of this GPU's rows
        one = torch.randn(WORLD * half, dtype=torch.bfloat16, device=device)
        two = [torch.randn(WORLD * half, dtype=torch.bfloat16, device=device) for _ in range(2)]
        m = [[half] * WORLD for _ in range(WORLD)]
        o1, o2 = torch.empty_like(one), [torch.empty_like(x) for x in two]
        one_way = timed(lambda: a2a([one], m, [o1]), n)
        kv_way = timed(lambda: a2a(two, m, o2), n)
        row = {}
        if new:  # the cross-attention through the Runtime's Ulysses, against the package's own crossing
            row["a2v_ulysses"] = timed(lambda: ulysses.exchange(vq, aq, aq, pg, sdpa, "bench a2v"), n)
            row["v2a_ulysses"] = timed(lambda: ulysses.exchange(aq, vq, vq, pg, sdpa, "bench v2a"), n)
            row["a2v_ulysses_sync"] = returns(lambda: ulysses.exchange(vq, aq, aq, pg, sdpa, "bench a2v"))
            for name, args in (("a2v", (vq, aq, aq)), ("v2a", (aq, vq, vq))):
                got, want = ulysses.exchange(*args, pg, sdpa, f"bench {name}"), package(*args)
                row[f"{name}_exact"] = bool(torch.equal(got, want))
        emit("cross", stage=stage, rows=mine, a2v_package=a2v, v2a_package=v2a,
             a2v_sync=returns(lambda: package(vq, aq, aq)),
             staged_a2v_bytes_ms=round(2 * one_way["ms"], 3), staged_v2a_bytes_ms=kv_way["ms"], **row)
        del vq, aq, one, two, o1, o2

    # meet: the per-call rendezvous, lockstep, then with GPU 1 2 ms late each call.
    meet = ulysses._rendezvous(pg)
    for late in (0.0, 0.002):
        dist.barrier()
        took = []
        for _ in range(200):
            if r == 1 and late:
                time.sleep(late)
            meet.sequence += 1
            t0 = time.perf_counter()
            meet.gather({"rows": 1, "at": 0, "site": "bench"})
            took.append(time.perf_counter() - t0)
        took.sort()
        emit("meet", late_ms=late * 1e3, mean_ms=round(sum(took) / len(took) * 1e3, 3),
             p50_ms=round(took[len(took) // 2] * 1e3, 3), p90_ms=round(took[int(len(took) * 0.9)] * 1e3, 3))
    exchange.close()
    dist.destroy_process_group()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
