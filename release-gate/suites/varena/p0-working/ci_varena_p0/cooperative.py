"""Two fresh ordinary CUDA processes, real native backing and one task-owned tmpfs ledger.

No mocked driver, memory limit, model download, daemon, shared default-cache mutation or
post-CUDA fork. Run only through the adjacent ordinary Cozy entry point on an owned GPU.
"""
from __future__ import annotations

import gc
import hashlib
from importlib.metadata import version
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile
import threading
import time
import traceback

MIB = 1 << 20


def emit(value):
    print(json.dumps(value, sort_keys=True), flush=True)


def sha(path):
    with Path(path).open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


class Actor:
    def __init__(self, role, config, directory):
        self.role = role
        self.process = subprocess.Popen(
            [sys.executable, "-I", __file__, role, json.dumps(config), str(directory)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.buffer = b""
        self.receipts = []
        self.ready = self.read()
        assert self.ready.get("ready"), self.ready

    def read(self):
        deadline = time.monotonic() + 30
        while True:
            while b"\n" not in self.buffer:
                remaining = deadline - time.monotonic()
                assert remaining > 0 and self.selector.select(remaining), "fixture protocol did not answer"
                data = os.read(self.process.stdout.fileno(), 65536)
                assert data, (self.role, self.process.poll())
                self.buffer += data
            line, self.buffer = self.buffer.split(b"\n", 1)
            try:
                value = json.loads(line)
            except ValueError:
                emit({"peer": self.role, "log": line.decode(errors="replace")})
                continue
            self.receipts.append(value)
            emit({"peer": self.role, "receipt": value})
            if "error" in value:
                error = RuntimeError(f"{self.role} child failed: {value['error']}")
                error.peer_failure = value
                raise error
            return value

    def call(self, command):
        self.process.stdin.write((command + "\n").encode())
        self.process.stdin.flush()
        return self.read()

    def close(self):
        if self.process.poll() is None:
            self.call("close")
            self.process.wait(timeout=30)
            assert self.process.returncode == 0


def child(role, config, directory):
    import tensorfs
    from tensorfs import _ext, varena
    from tensorfs.varena import Cooperation
    import tensorfs.varena.torch as vt
    import torch

    torch.set_num_threads(1)
    # No producer can fork concurrently before this metadata-only first initialization.
    Cooperation.initialize()
    actual = {"tensorfs_version": version("tensorfs"), "extension_sha256": sha(_ext.__file__),
              "shim_sha256": sha(Path(tensorfs.__file__).parent / "_varena_torch.so"),
              "torch_version": str(torch.__version__), "torch_git": torch.version.git_version}
    for key, value in actual.items():
        assert config[key] == value, (key, config[key], value)
    vt.install(0)
    warm = torch.ones(1, device="cuda")
    assert float(warm.cpu()[0]) == 1
    del warm
    torch.cuda.synchronize()
    torch.cuda.empty_cache()
    peer = Cooperation(0, config["gpu_uuid"], directory, priority=-10 if role == "donor" else 10)
    wrong = config["gpu_uuid"][:-1] + ("0" if config["gpu_uuid"][-1] != "0" else "1")
    try:
        Cooperation(0, wrong, directory)
    except RuntimeError as error:
        assert "UUID does not match" in str(error), error
    else:
        raise AssertionError("mismatched CUDA identity joined the table")
    owned = {}
    source_directory = tempfile.TemporaryDirectory(prefix="varena-peer-source-")
    checkpoint = None

    def figures():
        value = vt.stats(0)
        assert value["mapped_bytes"] == sum(value[k] for k in
            ("allocated_capacity", "pending_bytes", "free_bytes", "unassigned_bytes")), value
        return value

    def published(minimum=0):
        deadline = time.monotonic() + 15
        while True:
            row = peer.publish()
            assert row["physical"]["working"]["live"] + row["physical"]["graph_pools"]["live"] == figures()["allocated_capacity"]
            assert row["physical"]["total"] == sum(sum(v.values()) for k, v in row["physical"].items() if k != "total")
            if row["reclaimable"] >= minimum:
                return row
            assert time.monotonic() < deadline, {"completion_not_observed": row}
            time.sleep(0.001)

    def setup():
        nonlocal checkpoint
        assert not owned, "finish preceding owners before another setup"
        if checkpoint is None:
            writer = tensorfs.Writer(Path(source_directory.name) / "weights.cozytensors")
            component = writer.component("net")
            component.put("idle", torch.full((16 * MIB,), 11, dtype=torch.uint8))
            component.put("pinned", torch.full((8 * MIB,), 17, dtype=torch.uint8))
            checkpoint = writer.commit()
        arena = varena.Arena(0)
        regions = []
        for name, size in (("idle", 16 * MIB), ("pinned", 8 * MIB)):
            region = arena.reserve(size)
            read = region.prepare_read(checkpoint, [(f"net/{name}#value", 0, size)], threads=1)
            read.start()
            assert read.wait()["done"]
            del read
            regions.append(region)
        held = regions[1].pin()
        assert held is not None
        held.consume_on(torch.cuda.current_stream().cuda_stream)
        assert int(held.tensor.cpu()[0]) == 17
        live = torch.full((8 * MIB,), 19, device="cuda", dtype=torch.uint8)
        idle = torch.full((32 * MIB,), 31, device="cuda", dtype=torch.uint8)
        idle_pointer = idle.data_ptr()
        del idle
        torch.cuda.synchronize()
        owned.update(arena=arena, regions=regions, held=held, live=live, idle_pointer=idle_pointer)
        peer.publish(used=True)
        return published(48 * MIB)

    def cleanup():
        if "stream" in owned:
            owned["stream"].synchronize()
        owned.pop("held", None)
        owned.pop("live", None)
        owned.pop("output", None)
        gc.collect()
        torch.cuda.synchronize()
        torch.cuda.empty_cache()
        arena = owned.get("arena")
        if arena is not None:
            deadline = time.monotonic() + 15
            while arena.stats()["pinned_bytes"]:
                assert time.monotonic() < deadline, arena.stats()
                gc.collect()
                time.sleep(0.001)
            arena.reclaim(1 << 30)  # explicit fixture cleanup, outside the peer service measurement
        owned.clear()
        gc.collect()
        return published()

    emit({"ready": True, "pid": os.getpid(), "actual": actual, "table": peer.snapshot()})
    try:
        for line in sys.stdin:
            words = line.split()
            command = words[0]
            try:
                if command == "setup":
                    value = setup()
                elif command == "sample":
                    value = peer.sample()
                elif command == "ask":
                    value = peer.ask(int(words[1]), int(words[2]))
                elif command == "stale":
                    try:
                        peer.ask(int(words[1]), int(words[2]))
                    except RuntimeError as error:
                        assert "stale ledger capacity observation" in str(error), error
                        value = {"stale_sample_rejected": True}
                    else:
                        raise AssertionError("pre-release sample spent the deficit again")
                elif command == "snapshot":
                    value = peer.snapshot()
                elif command == "service":
                    before = figures()
                    value = peer.service()
                    after = figures()
                    assert after["context_joins"] == before["context_joins"]
                    assert after["event_waits"] == before["event_waits"]
                    if "held" in owned:
                        assert int(owned["held"].tensor.cpu()[0]) == 17
                        assert int(owned["live"].cpu()[0]) == 19
                        assert owned["regions"][1].resident
                    value["native_before"], value["native_after"] = before, after
                elif command == "pending":
                    stream = torch.cuda.Stream()
                    use = owned["regions"][0].pin()
                    assert use is not None
                    buffer = torch.full((32 * MIB,), 31, device="cuda", dtype=torch.uint8)
                    assert buffer.data_ptr() == owned["idle_pointer"], "pending arm did not reuse advertised backing"
                    output = torch.empty(48 * MIB, device="cuda", dtype=torch.uint8)
                    stream.wait_stream(torch.cuda.current_stream())
                    with torch.cuda.stream(stream):
                        torch.cuda._sleep(500_000_000)
                        output[:32 * MIB].copy_(buffer)
                        output[32 * MIB:].copy_(use.tensor)
                        done = torch.cuda.Event()
                        done.record(stream)
                    buffer.record_stream(stream)
                    use.consume_on(stream.cuda_stream)
                    del buffer, use
                    gc.collect()
                    owned.update(stream=stream, output=output, done=done)
                    before = figures()
                    assert not done.query(), "pending window was not exercised"
                    started = time.perf_counter()
                    value = peer.service()
                    value["service_seconds"] = time.perf_counter() - started
                    assert not done.query(), "peer service waited for submitted work"
                    after = figures()
                    assert after["context_joins"] == before["context_joins"]
                    assert after["event_waits"] == before["event_waits"]
                    assert owned["regions"][0].resident, "pending weight backing was released"
                    assert value["completed"] == 0 and value["pending"] >= 32 * MIB, value
                    value["event_pending_before_and_after"] = True
                elif command == "finish":
                    owned["stream"].synchronize()
                    output = owned["output"].cpu()
                    assert bool((output[:32 * MIB] == 31).all())
                    assert bool((output[32 * MIB:] == 11).all())
                    value = peer.service()
                    assert value["released"] > 0 and value["completed"] > 0, value
                    value["real_consumer_output_valid"] = True
                elif command == "capture":
                    graph = torch.cuda.CUDAGraph()
                    stream = torch.cuda.Stream()
                    with vt.graph(graph, stream=stream, reserve_bytes=2 * MIB):
                        before = peer.cuda_calls()
                        value = peer.service()
                        assert value["deferred"] and value["released"] == 0, value
                        assert peer.cuda_calls() == before, "capture peer service called CUDA"
                        owner = peer.snapshot()["owner"]
                        try:
                            peer.close()
                        except RuntimeError as error:
                            assert "capture" in str(error).lower(), error
                        else:
                            raise AssertionError("close during capture destroyed its owner")
                        assert peer.snapshot()["owner"] == owner
                        assert peer.cuda_calls() == before, "refused close called CUDA"
                        try:
                            Cooperation(0, config["gpu_uuid"], directory)
                        except RuntimeError as error:
                            assert "capture" in str(error).lower(), error
                        else:
                            raise AssertionError("participant open queried/joined during capture")
                        assert peer.cuda_calls() == before, "capture-time open queried CUDA identity"
                        captured = torch.ones(MIB, dtype=torch.uint8, device="cuda")
                    with varena.replay(0):
                        vt.replay(graph, stream=stream)
                        before = peer.cuda_calls()
                        replay = peer.service()
                        assert replay["deferred"] and replay["released"] == 0
                        assert peer.cuda_calls() == before, "replay-guard service called CUDA"
                    stream.synchronize()
                    assert int(captured.cpu()[0]) == 1
                    graph.reset()
                    del graph, captured
                    value["capture_and_replay_guard_cuda_calls"] = 0
                elif command == "owner-drop":
                    # Peer has retained this Arena already. Drop its external Python owners
                    # on another thread while CUDA work is pending and service detaches the GIL.
                    refs = [owned.pop("arena"), owned.pop("regions"), owned.pop("held")]
                    go = threading.Event()
                    times = {}
                    def drop_owners():
                        go.wait()
                        times["start"] = time.monotonic_ns()
                        refs.clear()
                        gc.collect()
                        times["end"] = time.monotonic_ns()
                    thread = threading.Thread(target=drop_owners)
                    thread.start()
                    stream = torch.cuda.Stream()
                    with torch.cuda.stream(stream):
                        torch.cuda._sleep(500_000_000)
                        done = torch.cuda.Event()
                        done.record(stream)
                    assert not done.query()
                    start = time.monotonic_ns()
                    go.set()
                    value = peer.service()
                    end = time.monotonic_ns()
                    thread.join(timeout=15)
                    assert not thread.is_alive()
                    assert not done.query(), "service/owner retirement context-joined unrelated work"
                    assert times["start"] <= end and times["end"] >= start, {"drop": times, "service": [start, end]}
                    stream.synchronize()
                    value["external_owner_drop_overlapped_service"] = True
                    value["unrelated_gpu_work_remained_pending"] = True
                    value["drop_ns"], value["service_ns"] = times, [start, end]
                elif command == "allocate":
                    before = peer.sample()
                    tensor = torch.full((int(words[1]),), 23, device="cuda", dtype=torch.uint8)
                    assert int(tensor.cpu()[0]) == int(tensor.cpu()[-1]) == 23
                    during = peer.sample()
                    del tensor
                    torch.cuda.synchronize()
                    torch.cuda.empty_cache()
                    value = {"actual_driver_before": before, "actual_driver_during": during,
                             "allocated_and_checked": int(words[1])}
                elif command == "gc-capture":
                    old_owner = peer.snapshot()["owner"]
                    sentinel = checkpoint.components["net"].tensors.read("idle", device="cuda")
                    graph = torch.cuda.CUDAGraph()
                    stream = torch.cuda.Stream()
                    with vt.graph(graph, stream=stream, reserve_bytes=2 * MIB):
                        before = figures()
                        owned.clear()  # enqueue external owners before the participant retirement
                        peer = None
                        gc.collect()
                        del sentinel  # this later cleanup task proves the queue advanced
                        gc.collect()
                        deadline = time.monotonic() + 15
                        while figures()["allocated_bytes"] > before["allocated_bytes"] - 24 * MIB:
                            assert time.monotonic() < deadline, figures()
                            time.sleep(0.001)
                        after = figures()
                        for key in ("driver_memory_calls", "event_queries", "event_records", "event_waits", "context_joins"):
                            assert after[key] == before[key], (key, before, after)
                        emit({"gc_active": True, "owner": old_owner, "later_cleanup_task_completed": True})
                        assert sys.stdin.readline().strip() == "finish-gc"
                        captured = torch.ones(MIB, device="cuda", dtype=torch.uint8)
                    vt.replay(graph, stream=stream)
                    stream.synchronize()
                    assert int(captured.cpu()[0]) == 1
                    graph.reset()
                    del graph, captured
                    gc.collect()
                    torch.cuda.synchronize()
                    torch.cuda.empty_cache()
                    deadline = time.monotonic() + 15
                    while True:
                        try:
                            peer = Cooperation(0, config["gpu_uuid"], directory, priority=-10)
                            break
                        except RuntimeError as error:
                            assert any(word in str(error).lower() for word in ("participant", "busy", "execution")), error
                            assert time.monotonic() < deadline, str(error)
                            time.sleep(0.001)
                    value = peer.publish()
                    assert value["physical"]["total"] == 0, value
                    value["previous_owner"], value["new_owner"] = old_owner, peer.snapshot()["owner"]
                    assert value["new_owner"] != old_owner
                    value["gc_retained_through_capture_then_released"] = True
                elif command == "retirement-capture":
                    # A real final Arena teardown waits for this submitted work. Capture
                    # attempts must observe the retirement guard; timing alone is no proof.
                    sentinel = checkpoint.components["net"].tensors.read("idle", device="cuda")
                    allocated_before = figures()["allocated_bytes"]
                    owned.pop("held", None)
                    owned.pop("live", None)
                    owned.pop("regions", None)
                    owned.pop("arena", None)
                    gc.collect()
                    del sentinel
                    gc.collect()
                    deadline = time.monotonic() + 15
                    while figures()["allocated_bytes"] > allocated_before - 24 * MIB:
                        assert time.monotonic() < deadline, figures()
                        time.sleep(0.001)
                    torch.cuda.synchronize()
                    torch.cuda.empty_cache()
                    pending_stream = torch.cuda.Stream()
                    with torch.cuda.stream(pending_stream):
                        torch.cuda._sleep(500_000_000)
                        done = torch.cuda.Event()
                        done.record(pending_stream)
                    assert not done.query()
                    errors = []
                    def close_peer():
                        try:
                            peer.close()
                        except BaseException as error:
                            errors.append(repr(error))
                    thread = threading.Thread(target=close_peer)
                    thread.start()
                    refused = False
                    deadline = time.monotonic() + 15
                    while thread.is_alive():
                        try:
                            with varena.capture(0):
                                pass  # native metadata guard; no CUDA graph work submitted
                        except RuntimeError as error:
                            assert "capture" in str(error).lower(), error
                            refused = True
                            break
                        assert time.monotonic() < deadline
                    thread.join(timeout=15)
                    assert not thread.is_alive() and not errors, errors
                    assert done.query(), "final arena close did not complete its ordinary teardown"
                    owned.clear()
                    torch.cuda.empty_cache()
                    peer = Cooperation(0, config["gpu_uuid"], directory, priority=-10)
                    value = peer.publish()
                    assert value["physical"]["total"] == 0, value
                    value["capture_refused_while_actual_retirement_in_progress"] = refused
                    value["capture_race_boundary"] = ("actual refusal observed" if refused else
                        "not exercised: capture attempt serialized behind teardown and entered after retirement")
                elif command == "unknown":
                    array = checkpoint.components["net"].tensors.read("idle", device="cuda")
                    tensor = torch.from_dlpack(array)
                    assert int(tensor.cpu()[0]) == 11
                    del tensor, array  # unannotated native/DLPack final owner: Unjoined
                    gc.collect()
                    deadline = time.monotonic() + 15
                    while figures()["pending_bytes"] < 16 * MIB:
                        assert time.monotonic() < deadline, figures()
                        time.sleep(0.001)
                    before = figures()
                    value = peer.service()
                    after = figures()
                    assert after["pending_bytes"] >= 16 * MIB and after["mapped_bytes"] >= 16 * MIB
                    assert after["context_joins"] == before["context_joins"]
                    assert after["event_waits"] == before["event_waits"]
                    assert value["pending"] == 0, "unknown consumers are not promised event-backed release"
                    value["unjoined_held_without_context_join"] = True
                elif command == "cleanup":
                    value = cleanup()
                elif command == "reopen":
                    previous = peer.snapshot()["owner"]
                    peer.close()
                    peer = Cooperation(0, config["gpu_uuid"], directory, priority=-10)
                    value = peer.publish()
                    assert value["physical"]["total"] == 0, value
                    value["previous_owner"] = previous
                    value["new_owner"] = peer.snapshot()["owner"]
                    assert value["new_owner"] != previous
                elif command == "close":
                    cleanup()
                    peer.close()
                    gc.collect()
                    torch.cuda.synchronize()
                    torch.cuda.empty_cache()
                    value = figures()
                    assert value["allocated_bytes"] == value["mapped_bytes"] == 0, value
                    emit(value)
                    break
                else:
                    raise AssertionError(command)
                emit(value)
            except BaseException as error:
                emit({"error": f"{type(error).__name__}: {error}", "traceback": traceback.format_exc(),
                      "command": command})
                raise
    finally:
        if checkpoint is not None:
            checkpoint.close()
        if peer is not None:
            peer.close()
        source_directory.cleanup()


def main(config):
    assert config.get("gpu_uuid", "").startswith(("GPU-", "MIG-")), "supply the actual typed device UUID"
    actors = []
    results = {}
    with tempfile.TemporaryDirectory(prefix="varena-gpu-ledger-", dir="/dev/shm") as directory:
        try:
            donor = Actor("donor", config, directory); actors.append(donor)
            requester = Actor("requester", config, directory); actors.append(requester)
            a, b = donor.ready["table"], requester.ready["table"]
            assert (a["backing_device"], a["backing_inode"], a["scope"]) == (b["backing_device"], b["backing_inode"], b["scope"])

            before = donor.call("setup")
            sample = requester.call("sample")
            round = requester.call(f"ask {sample['generation']} {48 * MIB}")
            early = requester.call("sample")
            given = donor.call("service")
            assert given["released"] >= 48 * MIB and not given["deferred"], given
            assert before["physical"]["total"] - given["accounting"]["physical"]["total"] == given["released"]
            assert given["accounting"]["physical"]["weights"]["live"] >= 8 * MIB
            assert given["accounting"]["physical"]["working"]["live"] >= 8 * MIB
            assert requester.call(f"stale {early['generation']} {48 * MIB}")["stale_sample_rejected"]
            duplicate = donor.call("service")
            assert duplicate["released"] == 0 and duplicate["completed"] == given["completed"]
            actual = requester.call(f"allocate {32 * MIB}")
            results["completed_exchange"] = {"before": before, "sample": sample, "round": round,
                "given": given, "duplicate": duplicate, "requester": actual}
            donor.call("cleanup")

            for arm in ("pending", "unknown", "capture", "owner-drop"):
                donor.call("setup")
                sample = requester.call("sample")
                ask = requester.call(f"ask {sample['generation']} {48 * MIB}")
                result = donor.call(arm)
                if arm == "pending": result["after_consumer_completion"] = donor.call("finish")
                if arm in ("capture", "owner-drop"):
                    result["after_guard_or_owner_completion"] = donor.call("service")
                results[arm] = {"ask": ask, "result": result}
                remaining = donor.call("cleanup")

            if remaining["reclaimable"]:
                sample = requester.call("sample")
                requester.call(f"ask {sample['generation']} {remaining['reclaimable']}")
                donor.call("service")
            results["explicit_close"] = donor.call("reopen")
            donor.call("setup")
            active_gc = donor.call("gc-capture")
            table = requester.call("snapshot")
            assert any(row["id"] == active_gc["owner"] for row in table["rows"]), table
            results["gc_during_capture"] = {"active": active_gc, "table_during_capture": table,
                                             "retired": donor.call("finish-gc")}
            donor.call("setup")
            results["retirement_capture_exclusion"] = donor.call("retirement-capture")

            donor.call("setup")
            sample = requester.call("sample")
            ask = requester.call(f"ask {sample['generation']} {48 * MIB}")
            assert not ask["finished"]
            donor.process.kill(); donor.process.wait(timeout=15)
            # ask performs an exact reap. Reusing this round returns its answer, never a new grant.
            gone = requester.call(f"ask {sample['generation']} {48 * MIB}")
            assert gone["generation"] == ask["generation"] and gone["completed"] == 0, gone
            assert all(d["answer"] == "OwnerGone" for d in gone["donations"]), gone
            observed = requester.call("sample")
            actual = requester.call(f"allocate {32 * MIB}")
            results["owner_death"] = {"round": gone, "subsequent_driver_observation": observed,
                                       "actual_allocation": actual, "reaping_credited_zero": True}
            requester.close()
            emit({"result": {"passed": True, "scope": "same-domain real GPU mechanism; no foreign-pressure latency or model claim",
                "cases": results, "gpu_uuid": config["gpu_uuid"]}})
        except BaseException as error:
            emit({"result": {"passed": False, "error": f"{type(error).__name__}: {error}",
                "traceback": traceback.format_exc(), "peer_failure": getattr(error, "peer_failure", None),
                "cases_completed": results, "receipts": {actor.role: actor.receipts for actor in actors},
                "gpu_uuid": config["gpu_uuid"]}})
            raise
        finally:
            for actor in actors:
                if actor.process.poll() is None:
                    actor.process.kill(); actor.process.wait(timeout=15)
                actor.selector.close()


if __name__ == "__main__":
    if len(sys.argv) == 2:
        main(json.loads(sys.argv[1]))
    else:
        child(sys.argv[1], json.loads(sys.argv[2]), sys.argv[3])
