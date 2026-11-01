"""Owned-process cancellation cell: a real pinned native weight is used until cancellation.

The parent gate cancels this exact Cozy run after the ready receipt. The finite deadline avoids
leaving a diagnostic process running if the controller disappears; it is not an eviction timer.
"""
import json
import os
import time
import sys
import threading

import torch
from tensorfs import varena
from tensorfs.varena import torch as vt


def exercise() -> None:
    torch.set_num_threads(1)
    vt.install(0)
    arena = varena.Arena(0)
    region = arena.reserve(16 << 20)
    use = region.prepare()
    use.tensor.fill_(17)
    use.commit()
    escaped = use.tensor[1024:]
    del use
    output = torch.empty_like(escaped)
    stream = torch.cuda.Stream()
    print(json.dumps({"cancel_ready": True, "pid": os.getpid(), "pinned_bytes": arena.stats()["pinned_bytes"]}), flush=True)
    stop = threading.Event()
    def stopping():
        sys.stdin.readline()
        stop.set()
    threading.Thread(target=stopping, daemon=True).start()
    deadline = time.monotonic() + 45
    steps = 0
    while time.monotonic() < deadline and not stop.is_set():
        with torch.cuda.stream(stream):
            torch.add(escaped, 1, out=output)
        stream.synchronize()
        assert output[-1].item() == 18
        steps += 1
        print(json.dumps({"cancel_step": steps, "pid": os.getpid()}), flush=True)
        time.sleep(0.1)
    if not stop.is_set():
        raise RuntimeError("the cancellation cell reached its finite deadline without cancellation")
    stream.synchronize()
    del output, escaped, region, arena
    torch.cuda.empty_cache()
    print(json.dumps({"result": {"passed": True, "cancel_cleanup": True, "steps": steps}}), flush=True)


if __name__ == "__main__":
    exercise()
