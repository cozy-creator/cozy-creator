"""Real current NCCL2/Working/Region client. Operator launches two or four ranks."""
from __future__ import annotations

import argparse
import gc
import hashlib
import json
import os
import traceback
from datetime import timedelta
from pathlib import Path

import torch
import torch.distributed as dist
from tensorfs import _ext
from tensorfs.varena import Source, api
from tensorfs.varena import torch as vt


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--gpu-uuids", required=True, help="JSON array in actual visible-device order")
parser.add_argument("--extension-sha256", required=True)
parser.add_argument("--core-sha256", required=True)
parser.add_argument("--shim-sha256", required=True)
parser.add_argument("--torch-adapter-sha256", required=True)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
rank = int(os.environ["RANK"])
device = int(os.environ["LOCAL_RANK"])
world = int(os.environ["WORLD_SIZE"])
assert world in (2, 4) and device == rank
uuids = json.loads(args.gpu_uuids)
assert len(uuids) == world and len(set(uuids)) == world and "UNBOUND" not in uuids
assert torch.__version__ == "2.15.0+cu132"
assert torch.version.git_version == "d0f2af8b66b20da4baa50644c0cd3f887a2eae2e"
assert os.environ.get("TORCH_DIST_USE_NCCL2") != "0"
assert not torch.cuda.is_initialized()


def check_file(path, expected):
    assert len(expected) == 64 and expected != "UNBOUND"
    actual = hashlib.sha256(Path(path).read_bytes()).hexdigest()
    assert actual == expected, (str(path), actual, expected)
    return {"path": str(path), "sha256": actual}


core_paths = {
    line.split()[-1]
    for line in Path("/proc/self/maps").read_text().splitlines()
    if line.split()[-1].endswith("/libvarena.so")
}
assert len(core_paths) == 1, core_paths
shim = Path(_ext.__file__).parent / "_varena_torch.so"
files = {
    "core": check_file(next(iter(core_paths)), args.core_sha256),
    "extension": check_file(_ext.__file__, args.extension_sha256),
    "shim": check_file(shim, args.shim_sha256),
    "adapter": check_file(vt.__file__, args.torch_adapter_sha256),
}
args.output.mkdir(parents=True, exist_ok=True)
receipt = args.output / f"rank-{rank}.json"
assert not receipt.exists(), "preserve earlier rank receipts"
report = {"passed": False, "rank": rank, "world": world, "pid": os.getpid(), "files": files, "stage": "activation"}


def save(stage):
    report["stage"] = stage
    receipt.write_text(json.dumps(report, indent=2) + "\n")


def snapshot():
    rows = [row for row in torch.cuda.memory_snapshot() if row["device"] == device]
    assert sum(row["total_size"] for row in rows) == vt.stats(device)["mapped_bytes"]
    for row in rows:
        assert sum(block["size"] for block in row["blocks"]) == row["total_size"]
    return rows


def covers(rows, value):
    return any(row["address"] <= value.data_ptr() and
               value.data_ptr() + value.numel() * value.element_size() <= row["address"] + row["total_size"]
               for row in rows)


def collective(value, offset):
    output = torch.empty_like(value)
    work = dist.all_to_all_single(output, value, async_op=True)
    work.wait()
    stream.synchronize()
    expected = torch.arange(world, dtype=torch.float32).repeat_interleave(16) + offset
    assert torch.equal(output.cpu(), expected)
    del work, output


save("activation")
vt.install(device)
torch.cuda.set_device(device)
actual_uuid = str(torch.cuda.get_device_properties(device).uuid)
normalize = lambda x: x.removeprefix("GPU-").replace("-", "").lower()
assert normalize(actual_uuid) == normalize(uuids[device])
report["gpu_uuid"] = actual_uuid
stream = torch.cuda.Stream(device=device)
gpu = api.open(device)
group = False
try:
    with torch.inference_mode(), torch.cuda.stream(stream):
        # The complete real inventory must seed NCCL registration, not only late maps.
        before_group = torch.full((196608,), rank + 1.0, device=device)  # 768KiB class leaves a real slab tail.
        stream.synchronize()
        report["before_group"] = snapshot()
        report["before_group_unassigned_bytes"] = vt.stats(device)["unassigned_bytes"]
        assert report["before_group_unassigned_bytes"] > 0
        assert covers(report["before_group"], before_group)
        save("group-construction")
        dist.init_process_group("nccl", timeout=timedelta(seconds=60), device_id=torch.device("cuda", device))
        group = True
        backend = dist.distributed_c10d._get_default_group()._get_backend(torch.device("cuda", device))
        assert type(backend).__name__ == "ProcessGroupNCCL2", type(backend)
        report["backend"] = type(backend).__name__
        collective(before_group[:world * 16], 1)
        del before_group
        stream.synchronize()
        gc.collect()
        vt.trim(device)
        save("late-map")
        prior = vt.stats(device)["mapped_bytes"]
        late = torch.full((16 << 20,), rank + 1.0, device=device)  # 64MiB actual new backing.
        stream.synchronize()
        assert vt.stats(device)["mapped_bytes"] > prior
        assert covers(snapshot(), late)
        collective(late[:world * 16], 1)

        save("region-input")
        source = Source.from_tensor(torch.full((world * 16,), rank + 10.0))
        region = gpu.region(source)
        used = gpu.use([region], stream.cuda_stream)
        used.wait_ready()
        foreign = used.tensor(dtype=source.dtype, shape=source.shape,
                              strides=source.strides, byte_offset=source.byte_offset)
        used.close()
        region.close()
        assert not covers(snapshot(), foreign), "Region must not be reported as Working backing"
        collective(foreign, 10)
        foreign.record_stream(stream)
        stream.synchronize()
        del foreign, used, region, source
        gc.collect()
        gpu.trim()

        save("graph-pool")
        pool = tuple(torch.cuda.graph_pool_handle())
        vt.prepare_pool(pool, 8 << 20, device)
        captured = torch.cuda.CUDAGraph()
        before_capture = vt.stats(device)["driver_memory_calls"]
        with vt.graph(captured, pool=pool, stream=stream, reserve_bytes=8 << 20):
            captured_output = late[:world * 16].clone()
        assert vt.stats(device)["driver_memory_calls"] == before_capture
        with vt.replay_scope(device):
            vt.replay(captured, stream=stream)
        stream.synchronize()
        collective(captured_output, 1)
        captured.reset()
        vt.trim(device)
        assert any(tuple(row["segment_pool_id"]) == pool for row in snapshot())
        del captured_output, captured, late
        stream.synchronize()
        gc.collect()
        vt.trim(device)
        assert not any(tuple(row["segment_pool_id"]) == pool for row in snapshot())
        report["after_pool_retirement"] = snapshot()
        save("group-destruction")
        dist.destroy_process_group()
        group = False
        del backend
        stream.synchronize()
        vt.trim(device)
        report.update(passed=True, scope="actual NCCL2 initialization/collectives/destruction, preexisting and late Working maps, Region input and graph-pool retirement; no symmetric-window or benchmark claim")
        save("complete")
except BaseException:
    report["error"] = traceback.format_exc()
    save("failed")
    raise
finally:
    cleanup_errors = []
    if group:
        try:
            dist.destroy_process_group()
        except BaseException:
            cleanup_errors.append(traceback.format_exc())
    try:
        gpu.close()
    except BaseException:
        cleanup_errors.append(traceback.format_exc())
    if cleanup_errors:
        report.update(passed=False, cleanup_errors=cleanup_errors)
        save("cleanup-failed")
        if "error" not in report:
            raise RuntimeError("native client cleanup failed; see retained rank receipt")
print(json.dumps({"passed": report["passed"], "rank": rank, "pid": os.getpid(), "gpu_uuid": actual_uuid, "receipt": str(receipt)}), flush=True)
