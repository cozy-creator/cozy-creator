"""Suite graphs' `graphs-operands` (var-034), run as root on the pod with an executor environment's Python: the machine's own
torch and TensorFS. A package can't run this: the package contract refuses `import tensorfs` and CUDA graphs.

    python operands.py <device> <case>

Records `a + b` (256 MiB each, float32) under `tensorfs.varena.torch.graph`, drops what `case` names, returns the working
allocator's free pages (`torch.cuda.empty_cache`), then replays raw, as a bench does (`graph.replay()`):
  outputs   the output dropped, the operands kept: the graph writes its own pool, which its reference keeps mapped
  operands  the operands dropped, the output kept: the graph reads pages that went back
Prints one JSON line: replayed (and the output's value), refused (varena's one line), or fault (the device's error).
"""

import gc
import json
import sys

import tensorfs
import tensorfs.varena.torch as vt
import torch

device, case = int(sys.argv[1]), sys.argv[2]
REFUSAL = "a tensor this CUDA graph reads was freed"
report: dict[str, object] = {"case": case, "tensorfs": getattr(tensorfs, "__version__", "?")}
try:
    vt.install(device)
    torch.cuda.set_device(device)
    n = 64 << 20
    operands = [torch.full((n,), 1.0, device="cuda"), torch.full((n,), 2.0, device="cuda")]

    def call() -> torch.Tensor:
        return torch.add(operands[0], operands[1])

    call()  # its kernels loaded before the capture
    torch.cuda.synchronize()
    graph = torch.cuda.CUDAGraph()
    with vt.graph(graph):
        out: torch.Tensor | None = call()
    torch.cuda.synchronize()
    report["operands_known"] = len(getattr(graph, "_varena_operands", ()))
    if case == "operands":
        operands.clear()
    else:
        out = None
    gc.collect()
    torch.cuda.synchronize()
    before = vt.stats(device)["mapped_bytes"]
    torch.cuda.empty_cache()
    report["released_mib"] = (before - vt.stats(device)["mapped_bytes"]) >> 20
    try:
        graph.replay()
        torch.cuda.synchronize()
        report["replayed"] = True
        if out is not None:
            report["value"] = float(out[: 1 << 20].mean())
    except RuntimeError as error:
        said = str(error).splitlines()[0][:300]
        report["refused" if said.startswith(REFUSAL) else "fault"] = said
except Exception as error:  # noqa: BLE001  (the report says what happened)
    report["error"] = f"{type(error).__name__}: {error}"[:400]
print(json.dumps(report), flush=True)
