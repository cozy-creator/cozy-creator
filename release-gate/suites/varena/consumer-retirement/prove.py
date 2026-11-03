"""Real one-GPU consumer retirement. Run each case in an owned fresh Cozy child."""
from __future__ import annotations

import argparse
import gc
import hashlib
import json
import os
from pathlib import Path
import threading
import time
import traceback

import torch
from cozy_runtime.internal import accel
from tensorfs import _ext
from tensorfs.varena import Source, api
from tensorfs.varena import _runtime as private
from tensorfs.varena import torch as vt

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--input", type=Path, required=True)
parser.add_argument("--case", required=True, choices=("private", "typed", "reuse", "capture", "template", "failed-template", "fill-abort", "capture-refusal"))
args = parser.parse_args()
binding = json.loads(args.input.read_text())
assert binding["gpu_uuid"] != "UNBOUND"
assert torch.__version__ == "2.15.0+cu132"
assert torch.version.git_version == "d0f2af8b66b20da4baa50644c0cd3f887a2eae2e"
assert not torch.cuda.is_initialized()
package = Path(_ext.__file__).parent
core_paths = {line.split()[-1] for line in Path("/proc/self/maps").read_text().splitlines()
              if line.split()[-1].endswith("/libvarena.so")}
assert len(core_paths) == 1
paths = {"core": Path(next(iter(core_paths))), "extension": Path(_ext.__file__),
         "shim": package / "_varena_torch.so", "torch_adapter": Path(vt.__file__),
         "private_adapter": Path(private.__file__), "runtime_accel": Path(accel.__file__),
         "runtime_domain": Path(accel.__file__).parents[1] / "graphs/domain.py",
         "runtime_loop": Path(accel.__file__).parents[1] / "internal/loop.py"}
files = {}
for name, path in paths.items():
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    assert actual == binding[name + "_sha256"], (name, actual, binding[name + "_sha256"])
    files[name] = {"path": str(path), "sha256": actual}
vt.install(0)
uuid = str(torch.cuda.get_device_properties(0).uuid)
normalize = lambda x: x.removeprefix("GPU-").replace("-", "").lower()
assert normalize(uuid) == normalize(binding["gpu_uuid"])
assert torch.cuda.device_count() == 1
report = {"passed": False, "case": args.case, "pid": os.getpid(), "gpu_uuid": uuid, "files": files}
owned = []


def stream():
    value = accel.nonblocking_stream(torch, 0)
    owned.append(value)
    return value


def close_streams(values):
    # Match the Runtime companion: no destruction until every retirement succeeds.
    handles = [value.cuda_stream for value in values]
    for value in values:
        value.synchronize()
        vt.retire_stream(value, 0)
    for value in values:
        accel.destroy_stream(torch, value)
        owned.remove(value)
    return handles


def make_cycle(value):
    cycle = [value]
    cycle.append(cycle)
    return [cycle]  # Reachable until the idle thread explicitly clears this root.


def idle_gc(holder):
    result = {}
    def collect():
        try:
            result["thread"] = threading.get_native_id()
            holder.clear()
            result["collected"] = gc.collect()
        except BaseException:
            result["error"] = traceback.format_exc()
    thread = threading.Thread(target=collect, name="consumer-idle-gc")
    thread.start()
    thread.join(60)
    assert not thread.is_alive(), "owned-child deadline required: idle GC did not finish"
    assert "error" not in result, result
    return result


def private_values(on, *, commit=True):
    arena = private.Arena(0)
    region = arena.reserve(4096)
    with torch.cuda.stream(on):
        use = region.prepare()
        assert use.needs_fill
        use.consume_on(on.cuda_stream)
        use.tensor.fill_(77)
        if commit:
            use.commit()
        canonical = use.tensor
        other = torch.from_dlpack(use._native)
        assert canonical.data_ptr() == other.data_ptr()
        assert canonical.untyped_storage()._cdata != other.untyped_storage()._cdata
        on.synchronize()
    return arena, region, use, canonical, other


with torch.inference_mode():
    if args.case in ("private", "reuse", "fill-abort"):
        source_stream = stream()
        arena, region, use, canonical, other = private_values(source_stream, commit=args.case != "fill-abort")
        alias = other[::2]
        del use, canonical, other
        old_handle, = close_streams([source_stream])
        if args.case == "fill-abort":
            holder = make_cycle(alias)
            del alias
            report["idle_gc"] = idle_gc(holder)
            later = stream()
            with torch.cuda.stream(later):
                refill = region.prepare()
                assert refill.needs_fill, "aborted contents became readable"
                refill.tensor.fill_(19)
                refill.consume_on(later.cuda_stream)
                refill.commit()
                assert torch.equal(refill.tensor.cpu(), torch.full((4096,), 19, dtype=torch.uint8))
                del refill
            close_streams([later])
        else:
            attempts = []
            for _ in range(32 if args.case == "reuse" else 1):
                later = stream()
                attempts.append(later.cuda_stream)
                if args.case != "reuse" or later.cuda_stream == old_handle:
                    break
                close_streams([later])
            else:
                raise RuntimeError("actual raw-handle reuse not observed; this case is UNQUALIFIED")
            report["old_handle"] = old_handle
            report["later_handles"] = attempts
            report["actual_handle_reuse"] = later.cuda_stream == old_handle
            with torch.cuda.stream(later):
                alias.record_stream(later)  # Must reach the private family's core pointer route.
                copied = alias.clone()
                later.synchronize()
                assert torch.equal(copied.cpu(), torch.full((2048,), 77, dtype=torch.uint8))
                del copied
            holder = make_cycle(alias)
            del alias
            close_streams([later])
            report["idle_gc"] = idle_gc(holder)
        released = arena.reclaim(1 << 30)
        assert released > 0 and not region.resident
        report["released_bytes"] = released

    elif args.case == "typed":
        source_stream = stream()
        gpu = api.open(0)
        source = Source.from_tensor(torch.full((4096,), 77, dtype=torch.uint8))
        region = gpu.region(source)
        used = gpu.use([region], source_stream.cuda_stream)
        used.wait_ready()
        tensor = used.tensor(dtype=source.dtype, shape=source.shape, strides=source.strides, byte_offset=source.byte_offset)
        used.close(); region.close()
        with torch.cuda.stream(source_stream):
            assert torch.equal(tensor.cpu(), torch.full((4096,), 77, dtype=torch.uint8))
        holder = make_cycle(tensor)
        del tensor, used, region
        close_streams([source_stream])
        report["idle_gc"] = idle_gc(holder)
        deadline = time.monotonic() + 60
        while api.retirement_stats()["queued"]:
            assert not api.retirement_stats()["quarantined"]
            if time.monotonic() >= deadline:
                raise TimeoutError("explicit retirement proof deadline")
            time.sleep(0.005)
        gpu.trim()
        assert gpu.stats()["weights_resident"] == 0
        assert api.retirement_stats()["quarantined"] == 0
        gpu.close()

    else:
        source_stream, control = stream(), stream()
        arena, region, use, canonical, other = private_values(source_stream)
        pool = tuple(torch.cuda.graph_pool_handle())
        keep = args.case in ("template", "failed-template")
        graph = torch.cuda.CUDAGraph(keep_graph=keep)
        fork, join = torch.cuda.Event(), torch.cuda.Event()
        reserve = 8 << 20
        failed = False
        captured_output = None
        try:
            with vt.graph(graph, pool=pool, stream=control, reserve_bytes=reserve):
                fork.record(control)
                source_stream.wait_event(fork)
                try:
                    with torch.cuda.stream(source_stream):
                        canonical.record_stream(source_stream)
                        captured_output = canonical + 1
                        # Base Use and a distinct Storage die during valid capture.
                        # The canonical Storage is an actual retained owner, not a count guess.
                        del use, other
                        gc.collect()
                        if args.case == "capture-refusal":
                            try:
                                vt.retire_stream(source_stream, 0)
                            except RuntimeError:
                                pass
                            else:
                                raise AssertionError("active capture retirement unexpectedly succeeded")
                            del canonical
                            gc.collect()  # Genuinely last owner: exact Pin must remain unproven.
                        if args.case == "failed-template":
                            torch.empty((32 << 20,), dtype=torch.uint8, device="cuda")
                finally:
                    join.record(source_stream)
                    control.wait_event(join)
        except torch.OutOfMemoryError:
            assert args.case == "failed-template"
            failed = True
        assert failed == (args.case == "failed-template")
        if args.case == "capture":
            with vt.replay_scope(0):
                vt.replay(graph, stream=source_stream)
            source_stream.synchronize()
            assert torch.equal(captured_output.cpu(), torch.full((4096,), 78, dtype=torch.uint8))
        # keep_graph=True cases deliberately remain template-only, matching the close path.
        graph.reset()
        del captured_output, graph
        if args.case != "capture-refusal":
            holder = make_cycle(canonical)
            del canonical
        close_streams([source_stream, control])
        if args.case != "capture-refusal":
            report["idle_gc"] = idle_gc(holder)
            released = arena.reclaim(1 << 30)
            assert released > 0 and not region.resident
            report["released_bytes"] = released
        else:
            assert arena.reclaim(1 << 30) == 0 and region.resident
            report["cleanup"] = "unowned capture Pin deliberately retained to child exit"
        report.update(keep_graph=keep, template_only=keep, expected_capture_failure=failed)

assert not owned
report.update(passed=True, scope="actual source aliases, checked stream retirement/destruction, shared private owner and idle-thread GC; no model performance or universal safety claim")
print(json.dumps(report, indent=2))
