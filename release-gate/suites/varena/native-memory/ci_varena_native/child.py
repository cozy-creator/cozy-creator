"""One stage of the native-memory cell in a process of its own: a standalone varena Memory (no TensorD lease) on this
process's GPU, beside the ballast process that holds the rest of the card. Prints a JSON line per step, the result
last, then exits without teardown (a faulted context must not hang the cell).

  recovery  a capacity refusal is torch's OutOfMemoryError and evicts nothing; a usage error is an ordinary error;
            a demand past free bytes takes cold weights, then an unpinned cache region (swapped to RAM), and the
            weights refill from the store with the same output; the region pins again not fresh, its bytes intact
  fault     a kernel reads an evicted region outside its pin: the fault and every allocation after it are
            AcceleratorError, never OutOfMemoryError, and nothing is served again
"""

import hashlib
import json
import os
import sys
import tempfile
import time

import tensorfs
import torch
from tensorfs.varena.torch import Declared, Memory, Weights
from torch import nn

MIB = 1 << 20
RESERVE = 512 * MIB  # varena's per-GPU reserve (backend.rs RESERVE)
D, BLOCKS = 4096, 32  # 32 blocks of 32 MiB fp16 weights
CACHE = 512 * MIB


def say(**facts: object) -> None:
    print(json.dumps(facts), flush=True)


def left() -> int:
    """What one more physical allocation may take now: the driver's free bytes less the reserve."""
    return torch.cuda.mem_get_info()[0] - RESERVE


def digest(t: torch.Tensor) -> str:
    return hashlib.sha256(t.cpu().numpy().tobytes()).hexdigest()


def failure(e: BaseException) -> dict[str, object]:
    return {"type": f"{type(e).__module__}.{type(e).__name__}", "oom": isinstance(e, torch.OutOfMemoryError),
            "error_code": getattr(e, "error_code", None), "message": str(e).splitlines()[0][:300]}


def weights(mem: Memory) -> int:
    return sum(mem.classes().values())


def mapped(mem: Memory) -> int:
    return int(mem.torch_stats()["mapped"])  # type: ignore[call-overload]


def recovery(seed: int) -> dict[str, object]:
    mem = Memory.open(0)
    out: dict[str, object] = {"standalone_left_at_open": left()}
    g = torch.Generator().manual_seed(seed)
    with tempfile.TemporaryDirectory() as tmp:
        with tensorfs.Writer(os.path.join(tmp, "net.cozytensors")) as w:
            net_w = w.component("net")
            for i in range(BLOCKS):
                net_w.put(f"{i}.weight", (torch.randn(D, D, generator=g) / 64).half())
        ck = w.commit()
        with torch.device("meta"):
            net = nn.Sequential(*(nn.Linear(D, D, bias=False, dtype=torch.float16) for _ in range(BLOCKS)))
        vars(net)["_varena"] = Declared(Weights(ck, "net"), blocks=[str(i) for i in range(BLOCKS)])
        mem.load(net, "gpu")
        x = torch.randn(8, D, generator=g).half().cuda()

        def forward(tag: str) -> torch.Tensor:
            with mem.request(tag, plan=[net]), torch.no_grad():
                y = net(x).cpu()
            return y

        y0 = forward("warm")
        out["weights_resident"] = w_bytes = weights(mem)
        say(step="loaded", weights=w_bytes, left=left())

        cache = mem.region("native-memory", "latent", spill=True)
        with cache.allocating():
            c = torch.randint(0, 256, (CACHE,), dtype=torch.uint8, device="cuda")
            c0 = digest(c)
        say(step="cached", left=left(), mapped=mapped(mem))

        # More than evicting everything could give: refused at once, nothing taken.
        before = (weights(mem), mapped(mem))
        need = left() + w_bytes + CACHE + 256 * MIB
        try:
            torch.empty(need, dtype=torch.uint8, device="cuda")
            out["refusal"] = {"bytes": need, "allocated": True}
        except Exception as e:  # noqa: BLE001 - the class is the fact recorded
            out["refusal"] = {"bytes": need, **failure(e)}
        out["refusal_kept"] = {"weights": weights(mem) == before[0], "mapped": mapped(mem) == before[1]}
        say(step="refused", **out["refusal_kept"])  # type: ignore[arg-type]

        try:
            with torch.cuda.stream(torch.cuda.ExternalStream(2)):  # cudaStreamPerThread
                torch.empty(MIB, dtype=torch.uint8, device="cuda")
            out["usage"] = {"allocated": True}
        except Exception as e:  # noqa: BLE001
            out["usage"] = failure(e)

        # Past free bytes: cold weights first, then the unpinned cache (swapped), never a refusal.
        need = left() + w_bytes + CACHE // 2
        start = time.perf_counter()
        try:
            a = torch.empty(need, dtype=torch.uint8, device="cuda")
            out["pressure"] = {"bytes": need, "ms": round((time.perf_counter() - start) * 1e3, 1), "allocated": True,
                               "weights_after": weights(mem), "mapped_after": mapped(mem)}
            del a
        except Exception as e:  # noqa: BLE001
            out["pressure"] = {"bytes": need, **failure(e)}
        torch.cuda.synchronize()
        say(step="pressed", **out["pressure"])  # type: ignore[arg-type]

        start = time.perf_counter()
        y1 = forward("refill")
        out["refill"] = {"ms": round((time.perf_counter() - start) * 1e3, 1), "same_output": bool(torch.equal(y0, y1)),
                         "weights": weights(mem)}
        with cache.pin() as fresh:
            out["region"] = {"fresh": fresh, "same_bytes": digest(c) == c0, "sha256": c0}
        cache.close()
    p, r = out["pressure"], out["refusal"]
    out["passed"] = bool(
        r.get("oom") and "exhausted" in str(r.get("message")) and all(out["refusal_kept"].values())  # type: ignore[union-attr]
        and not out["usage"].get("oom") and "allocated" not in out["usage"]  # type: ignore[union-attr]
        and p.get("allocated") and p.get("weights_after", w_bytes) < w_bytes  # type: ignore[union-attr]
        and out["refill"]["same_output"] and out["region"]["fresh"] is False and out["region"]["same_bytes"]  # type: ignore[index]
    )
    return out


def fault() -> dict[str, object]:
    mem = Memory.open(0)
    out: dict[str, object] = {"standalone_left_at_open": left()}
    cache = mem.region("native-memory-fault", "latent", spill=True)
    with cache.allocating():
        c = torch.ones(CACHE // 4, dtype=torch.float32, device="cuda")
    a = torch.empty(left() + CACHE // 2, dtype=torch.uint8, device="cuda")  # evicts the region
    del a
    torch.cuda.synchronize()
    try:
        out["read_unpinned"] = {"value": float(c.sum().item())}  # its addresses are unmapped now
    except Exception as e:  # noqa: BLE001
        out["read_unpinned"] = failure(e)
    time.sleep(1)  # the epoch poller meets the failed stream too; whichever saw it first, the classes must hold
    allocs = []
    for n in (64 * MIB, 96 * MIB, MIB):
        try:
            torch.empty(n, dtype=torch.uint8, device="cuda")
            allocs.append({"bytes": n, "allocated": True})
        except Exception as e:  # noqa: BLE001
            allocs.append({"bytes": n, **failure(e)})
    out["after_fault"] = allocs
    accel = [x.get("type") == "torch.AcceleratorError" and not x.get("oom") for x in allocs]
    out["passed"] = bool(out["read_unpinned"].get("type") == "torch.AcceleratorError" and all(accel))  # type: ignore[union-attr]
    return out


def main() -> None:
    stage, seed = sys.argv[1], int(sys.argv[2])
    try:
        result = recovery(seed) if stage == "recovery" else fault()
    except Exception as e:  # noqa: BLE001
        result = {"passed": False, "error": failure(e)}
    say(result=result)
    os._exit(0)


if __name__ == "__main__":
    main()
