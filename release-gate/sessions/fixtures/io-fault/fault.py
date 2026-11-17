"""cr-318's fault case (sessions/io-fault.sh), run as root on the pod with an executor environment's Python: its torch, and
the TensorFS the machine runs.

    python fault.py <device> <rounds>

Two threads read 64 tensors of 4 MiB each (a run of two staged pieces each) to the GPU at once from one checkpoint, so
one thread's runs are in flight while the other's run. Each round the private hook fails one staged piece, a reader
failing mid-run. Every read that did not fail must hold its tensor's exact bytes (sha256 against the host's), and each
failed one, read again after, too: no failed run's staging slot crossed into another run. Prints one JSON report.
"""

import hashlib
import json
import os
import sys
import threading
import time

import tensorfs
import torch
from tensorfs import _ext

N, SIZE = 64, 4 << 20
device, rounds = int(sys.argv[1]), int(sys.argv[2])
cuda = f"cuda:{device}"
source = torch.Generator().manual_seed(318)
data = {f"{side}{i}": torch.randint(0, 256, (SIZE,), dtype=torch.uint8, generator=source) for side in "ab" for i in range(N)}
want = {key: hashlib.sha256(value.numpy().tobytes()).hexdigest() for key, value in data.items()}
path = "/tmp/io-fault/fault.cozytensors"
os.makedirs(os.path.dirname(path), exist_ok=True)
if not os.path.exists(path):
    with tensorfs.Writer(path) as writer:
        component = writer.component("model", cls=("", "", ""), config={})
        for key, value in data.items():
            component.put(key, value)
tensors = tensorfs.open(path, local_files_only=True).components["model"].tensors
out = {key: torch.empty(SIZE, dtype=torch.uint8, device=cuda) for key in data}


def got(key):
    return hashlib.sha256(out[key].cpu().numpy().tobytes()).hexdigest()


def side(name, log):
    for i in range(N):
        key, began = f"{name}{i}", time.monotonic()
        try:
            tensors.read_into(key, out[key])
            log.append((key, began, time.monotonic(), ""))
        except Exception as error:  # the injected fault, as a read's own failure
            log.append((key, began, time.monotonic(), f"{type(error).__name__}: {error}"[:200]))


report = {"tensorfs": getattr(tensorfs, "__version__", "?"), "from": os.path.dirname(tensorfs.__file__), "rounds": []}
for r in range(rounds):
    for value in out.values():
        value.zero_()
    torch.cuda.synchronize(device)
    _ext._io_fail_pieces(16 + 9 * r, 1)
    logs = {"a": [], "b": []}
    threads = [threading.Thread(target=side, args=(name, logs[name])) for name in "ab"]
    began = time.monotonic()
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    seconds = time.monotonic() - began
    _ext._io_fail_pieces(0, 0)
    torch.cuda.synchronize(device)
    rows = logs["a"] + logs["b"]
    failed = [row for row in rows if row[3]]
    wrong = [row[0] for row in rows if not row[3] and got(row[0]) != want[row[0]]]
    # A failed read in flight while the other thread's run was: their intervals overlap.
    overlapped = [
        f[0] for f in failed
        if any(o[1] < f[2] and f[1] < o[2] for o in rows if o[0][0] != f[0][0])
    ]
    again = []
    for row in failed:
        tensors.read_into(row[0], out[row[0]])
        torch.cuda.synchronize(device)
        again.append(got(row[0]) == want[row[0]])
    report["rounds"].append({
        "failed": [row[0] for row in failed], "errors": sorted({row[3] for row in failed}),
        "overlapped": overlapped, "wrong": wrong, "reread_ok": all(again), "reads": len(rows),
        "seconds": round(seconds, 3), "gb_s": round(len(rows) * SIZE / seconds / 1e9, 2),
    })
ok = all(r["failed"] and not r["wrong"] and r["reread_ok"] for r in report["rounds"])
report["pass"] = ok and any(r["overlapped"] for r in report["rounds"])
report["overlapped_rounds"] = sum(1 for r in report["rounds"] if r["overlapped"])
print(json.dumps(report))
