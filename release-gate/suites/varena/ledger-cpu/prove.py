"""Real Linux process/fork/OFD/shared-publication proof. No CUDA, model, daemon or rental."""
from pathlib import Path
import argparse
import ctypes
import errno
import fcntl
import hashlib
import json
import mmap
import os
import selectors
import signal
import struct
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument("peer", type=Path, help="cargo-built varena ledger_peer example")
parser.add_argument("output", type=Path, help="new receipt directory")
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
peer_binary = args.peer.resolve()
actors = []
events = []


class Peer:
    def __init__(self, name, directory, role="peer", priority=0):
        self.name = name
        self.stderr = (args.output / (name + ".stderr")).open("w")
        self.process = subprocess.Popen([str(peer_binary), role, str(directory), str(priority)],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr)
        self.buffer = b""
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        actors.append(self)

    def read(self, timeout=15):
        deadline = time.monotonic() + timeout
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            assert remaining > 0, f"{self.name}: proof protocol did not answer; not a death inference"
            assert self.selector.select(remaining), f"{self.name}: proof protocol timeout"
            data = os.read(self.process.stdout.fileno(), 65536)
            assert data, f"{self.name}: exited {self.process.poll()}; see stderr"
            self.buffer += data
        line, self.buffer = self.buffer.split(b"\n", 1)
        result = json.loads(line)
        events.append({"peer": self.name, "result": result})
        return result

    def send(self, command):
        self.process.stdin.write((command + "\n").encode())
        self.process.stdin.flush()

    def call(self, command):
        self.send(command)
        return self.read()

    def close(self):
        if self.process.poll() is None:
            self.send("close")
            self.process.wait(timeout=15)
        assert self.process.returncode == 0


def inspected(directory):
    result = subprocess.run([str(peer_binary), "inspect", str(directory)], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


class Flock(ctypes.Structure):
    _fields_ = [("kind", ctypes.c_short), ("whence", ctypes.c_short),
                ("start", ctypes.c_longlong), ("length", ctypes.c_longlong), ("pid", ctypes.c_int)]


def publication_held(fd):
    lock = Flock(fcntl.F_WRLCK, os.SEEK_SET, 0, 1, 0)
    try:
        fcntl.fcntl(fd, fcntl.F_OFD_SETLK, bytes(lock))
    except OSError as error:
        if error.errno in (errno.EAGAIN, errno.EACCES):
            return True
        raise
    lock.kind = fcntl.F_UNLCK
    fcntl.fcntl(fd, fcntl.F_OFD_SETLK, bytes(lock))
    return False


def stale_samples(directory):
    """A real peer's newer sample must not spend an older round's release again."""
    directory.mkdir()
    donor = Peer("sample-donor", directory)
    first = Peer("sample-first", directory)
    second = Peer("sample-second", directory)
    for actor in (donor, first, second):
        actor.read()
    assert donor.call("publish 0 100 0 100 1")["ok"]
    old = first.call("sample 10000 1000 1")["sample"]
    active = first.call(f"ask {old} 80")["round"]
    early = second.call("sample 10000 1000 2")["sample"]
    assert second.call(f"ask {early} 80")["round"] == active
    # No physical-row publish here: acknowledgement alone must invalidate the early sample.
    assert donor.call(f"ack {active} 50 0 complete")["ok"]
    stale = second.call(f"ask {early} 80")
    assert "stale" in stale.get("error", ""), {"pre_release_sample_opened_new_round": stale}
    after = first.call("snapshot")
    assert after["round"]["id"] == active and after["round"]["completed"] == 50
    assert after["sample"]["id"] == early and after["sample"]["used"] == 1000
    assert first.call(f"ask {old} 999")["round"] == active
    epoch = after["capacity_epoch"]
    second.call("sample 10000 950 3")
    before_duplicate = first.call("snapshot")
    assert donor.call(f"ack {active} 50 0 complete")["ok"]
    assert first.call("snapshot") == before_duplicate
    # An identical ack must not make an observation/commit pair spuriously stale.
    fresh = second.call(f"sample-at {before_duplicate['generation']} 10000 950 3")["sample"]
    next_round = second.call(f"ask {fresh} 40")["round"]
    assert next_round != active
    assert donor.call(f"ack {next_round} 0 0 refused")["ok"]

    before_publish = first.call("sample 10000 950 4")["sample"]
    assert donor.call("publish 0 100 0 100 1")["ok"]  # even equal counts are a new physical observation
    assert "stale" in first.call(f"ask {before_publish} 40")["error"]
    assert first.call("snapshot")["capacity_epoch"] > epoch

    observed = second.call("snapshot")["generation"]
    assert donor.call("publish 0 50 0 50 5")["ok"]
    assert "stale" in second.call(f"sample-at {observed} 10000 950 5")["error"]
    assert second.call("snapshot")["sample"]["id"] == before_publish

    # Independent writers observed one generation before either commits its driver sample.
    expected = first.call("snapshot")["generation"]
    first.send(f"sample-at {expected} 10000 950 6")
    second.send(f"sample-at {expected} 10000 950 6")
    competing = [first.read(), second.read()]
    assert sum("sample" in result for result in competing) == 1, competing
    assert sum("stale" in result.get("error", "") for result in competing) == 1, competing
    accepted = next(result["sample"] for result in competing if "sample" in result)
    final_round = first.call(f"ask {accepted} 40")["round"]
    assert final_round != next_round
    before_pending = first.call("snapshot")["capacity_epoch"]
    assert donor.call(f"ack {final_round} 0 40 pending")["ok"]
    pending_snapshot = second.call("snapshot")
    assert donor.call(f"ack {final_round} 0 40 pending")["ok"]
    assert second.call("snapshot") == pending_snapshot
    assert pending_snapshot["capacity_epoch"] == before_pending
    assert donor.call(f"ack {final_round} 20 20 pending")["ok"]
    after_partial = second.call("snapshot")["capacity_epoch"]
    assert after_partial > before_pending
    assert donor.call(f"ack {final_round} 20 20 pending")["ok"]
    assert second.call("snapshot")["capacity_epoch"] == after_partial
    assert donor.call(f"ack {final_round} 40 0 complete")["ok"]
    final = first.call("snapshot")
    for actor in (donor, first, second):
        actor.close()
    return {"early_sample": early, "completed_round": active,
            "pre_release_sample_refused": True, "unfinished_round_joined": True,
            "physical_publication_invalidates": True, "in_flight_sample_commit_refused": True,
            "concurrent_sample_commit_once": True, "duplicate_acks_do_not_advance_capacity": True,
            "pending_is_not_capacity": True, "fresh_sample_opens_next_round": True, "final": final}


report = {"passed": False, "binary": str(peer_binary),
          "binary_sha256": hashlib.sha256(peer_binary.read_bytes()).hexdigest(),
          "cuda_initialized": False, "scope": "metadata/process proof only; published byte counts are fixture observations, not actual GPU releases"}
fork_directory = None
fork_pidfd = None
try:
    with tempfile.TemporaryDirectory(prefix="varena-ledger-proof-", dir="/dev/shm") as temporary:
        root = Path(temporary)
        report["stale_samples"] = stale_samples(root / "stale-samples")
        basic = root / "basic"
        basic.mkdir()
        donor = Peer("donor", basic, priority=-10)
        donor_ready = donor.read()
        report["close_on_exec"] = donor.call("exec-check")
        report["descriptor_bound"] = donor.call("descriptor-limit")
        assert donor.call("threads")["consistent"]
        assert donor.call("publish 0 100 0 100 1")["ok"]
        first = Peer("asker-one", basic, priority=10)
        first_ready = first.read()
        second = Peer("asker-two", basic, priority=20)
        second_ready = second.read()
        assert len({row["snapshot"]["inode"] for row in (donor_ready, first_ready, second_ready)}) == 1
        sample = first.call("sample 10000 1000 1")["sample"]
        first.send(f"ask {sample} 80")
        second.send(f"ask {sample} 120")
        a, b = first.read(), second.read()
        assert a["round"] == b["round"] and a["requested"] == b["requested"] in (80, 120)
        round_id = a["round"]
        assert donor.call(f"ack {round_id} 40 20 pending")["ok"]
        assert donor.call(f"ack {round_id} 40 20 pending")["ok"]
        pending = first.call("snapshot")
        assert pending["round"]["completed"] == 40 and not pending["round"]["finished"]
        assert donor.call("publish 0 50 0 50 2")["ok"]
        assert donor.call(f"ack {round_id} 50 0 complete")["ok"]
        assert donor.call(f"ack {round_id} 50 0 complete")["ok"]
        assert first.call(f"ask {sample} 999")["round"] == round_id
        done = first.call("snapshot")
        assert done["round"]["completed"] == 50 and done["round"]["finished"]

        os.kill(donor.process.pid, signal.SIGSTOP)
        stopped = os.waitpid(donor.process.pid, os.WUNTRACED)
        assert os.WIFSTOPPED(stopped[1])
        assert first.call("reap")["reaped"] == []
        assert any(row["pid"] == donor.process.pid for row in first.call("snapshot")["rows"])
        os.kill(donor.process.pid, signal.SIGCONT)
        donor.process.kill()
        donor.process.wait(timeout=15)
        assert first.call("reap")["reaped"] == [donor.process.pid]
        reaped = first.call("snapshot")
        assert reaped["sample"]["used"] == 1000 and reaped["round"]["completed"] == 50
        replacement = Peer("replacement", basic)
        replaced = replacement.read()
        assert replaced["slot"] == donor_ready["slot"] and replaced["incarnation"] != donor_ready["incarnation"]
        assert "stale" in replacement.call(f"ack {round_id} 50 0 complete")["error"]
        report["asks_and_liveness"] = {"concurrent_round": round_id, "pending_not_credited": True,
            "duplicate_acks_counted_once": True, "stopped_owner_retained": True,
            "reap_did_not_credit_memory": True, "stale_incarnation_rejected": True,
            "thread_publication": True, "snapshot": reaped}
        assert replacement.call("publish 0 25 0 25 3")["ok"]
        next_sample = first.call("sample 10000 999 2")["sample"]
        abandoned_round = first.call(f"ask {next_sample} 25")["round"]
        before_death_sample = second.call("sample 10000 999 2")["sample"]
        first.process.kill()
        first.process.wait(timeout=15)
        assert "stale" in second.call(f"ask {before_death_sample} 25")["error"]
        assert second.call("reap")["reaped"] == [], "stale sample rejection must not roll back ask's reap"
        abandoned = second.call("snapshot")
        assert all(row["pid"] != first.process.pid for row in abandoned["rows"])
        assert abandoned["round"]["aborted"] and abandoned["round"]["completed"] == 0
        assert abandoned["sample"]["used"] == 999
        assert "stale" in replacement.call(f"ack {abandoned_round} 0 0 refused")["error"]
        fresh_sample = second.call("sample 10000 800 3")["sample"]
        resumed_round = second.call(f"ask {fresh_sample} 25")["round"]
        assert resumed_round != abandoned_round
        assert replacement.call(f"ack {resumed_round} 0 0 refused")["ok"]
        assert second.call("snapshot")["round"]["finished"]
        report["round_owner_death"] = {"aborted_round":abandoned_round,"new_round":resumed_round,
                                       "no_release_invented":True,"fresh_sample_required":True,
                                       "stale_ask_keeps_exact_reap":True}
        replacement.close()
        second.close()
        assert inspected(basic)["rows"] == []

        capacity = root / "row-capacity"
        capacity.mkdir()
        bounded = [Peer(f"row-{index}", capacity) for index in range(64)]
        ready_rows = [actor.read() for actor in bounded]
        assert len({row["slot"] for row in ready_rows}) == 64
        full = subprocess.run([str(peer_binary), "peer", str(capacity)], input="", capture_output=True, text=True, timeout=15)
        assert full.returncode != 0 and "capacity exhausted: participant rows" in full.stderr
        for actor in bounded:
            actor.close()
        assert inspected(capacity)["rows"] == []
        report["row_bound"] = {"independent_processes":64,"next_registration_refused":True,"all_closed":True}

        fork_directory = root / "fork"
        fork_directory.mkdir()
        parent = Peer("fork-family", fork_directory, role="fork")
        messages = [parent.read(), parent.read()]
        child = next(row for row in messages if "fork_child" in row)
        father = next(row for row in messages if "fork_parent" in row)
        assert child["fd_reuse"] and child["inherited_rejected"]
        assert len(child["snapshot"]["rows"]) == 2, "child close must not unlock its live parent's row"
        fork_pidfd = os.pidfd_open(child["fork_child"])
        watcher = Peer("fork-watcher", fork_directory)
        watcher.read()
        parent.process.kill()
        parent.process.wait(timeout=15)
        assert watcher.call("reap")["reaped"] == [father["fork_parent"]]
        surviving = watcher.call("snapshot")
        assert any(row["pid"] == child["fork_child"] for row in surviving["rows"])
        (fork_directory / "child-stop").touch()
        wait = selectors.DefaultSelector()
        wait.register(fork_pidfd, selectors.EVENT_READ)
        assert wait.select(15), "child stop protocol did not complete; not a timeout-as-death decision"
        wait.close()
        watcher.close()
        report["fork"] = {"parent_death_child_survives": True, "reused_fd_not_double_closed": True,
                          "fresh_child_registration": True, "live_parent_lock_not_unlocked": True,
                          "parent": father, "child": child}

        racing = root / "fork-race"
        racing.mkdir()
        race = Peer("fork-open-close", racing, role="fork-race")
        race_result = race.read()
        race.process.wait(timeout=15)
        assert race.process.returncode == 0 and race_result["passed"]
        report["fork_after_single_threaded_initialization"] = race_result

        interrupted = root / "interrupted"
        interrupted.mkdir()
        writer = Peer("writer", interrupted)
        ready = writer.read()
        writer.send("burst")
        writer.read()
        path = Path(ready["path"])
        assert path.resolve().is_relative_to(root.resolve()) and not path.is_symlink()
        # Observe the actual library writer, not a replacement publication implementation.
        # Stop only while its OFD publication lock is held AND the inactive bank contains
        # newer row data than the still-committed bank. Then kill it before the commit store.
        with path.open("r+b", buffering=0) as file, mmap.mmap(file.fileno(), 0, access=mmap.ACCESS_READ) as mapped:
            bank_words = (len(mapped) // 8 - 16) // 2
            observed = None
            for attempt in range(1000):
                os.kill(writer.process.pid, signal.SIGSTOP)
                assert os.WIFSTOPPED(os.waitpid(writer.process.pid, os.WUNTRACED)[1])
                selector = struct.unpack_from("=Q", mapped, 15 * 8)[0]
                bank = selector & 1
                live_word = 17 + ready["slot"] * 20 + 7  # ledger layout v2 metadata prefix
                current = struct.unpack_from("=Q", mapped, (16 + bank * bank_words + live_word) * 8)[0]
                uncommitted = struct.unpack_from("=Q", mapped, (16 + (1 - bank) * bank_words + live_word) * 8)[0]
                if publication_held(file.fileno()) and uncommitted > current:
                    observed = {"attempt": attempt, "generation": selector >> 1,
                                "committed_value": current, "uncommitted_value": uncommitted}
                    break
                os.kill(writer.process.pid, signal.SIGCONT)
                time.sleep(0.0005)
            assert observed is not None, "did not intercept an actual uncommitted publication"
            writer.process.kill()
            writer.process.wait(timeout=15)
        recovered = inspected(interrupted)
        assert recovered["generation"] == observed["generation"]
        assert recovered["rows"][0]["live"] == observed["committed_value"]
        next_writer = Peer("after-kill", interrupted)
        next_writer.read()  # registration reaps the dead row and overwrites the inactive bank
        assert next_writer.call("publish 7 11 0 11 3")["ok"]
        assert next_writer.call("snapshot")["rows"][0]["idle"] == 11
        next_writer.close()
        report["interrupted_publication"] = {**observed, "old_bank_retained": True, "next_writer_recovered": True}

        namespace = subprocess.run(["unshare", "--user", "--map-current-user", "--mount",
                                    str(peer_binary), "inspect", str(basic)], capture_output=True, text=True)
        if "Operation not permitted" in namespace.stderr:
            report["namespace_transition"] = {"executed": False, "reason": namespace.stderr.strip()}
        else:
            assert namespace.returncode != 0 and "incompatible ledger" in namespace.stderr, namespace
            report["namespace_transition"] = {"executed": True, "mismatch_refused": True, "stderr": namespace.stderr.strip()}
        # Layout compatibility is a real on-disk negative control, separate from namespaces.
        ledger_path = next(basic.iterdir())
        with ledger_path.open("r+b", buffering=0) as file:
            original = os.pread(file.fileno(), 8, 8)
            try:
                os.pwrite(file.fileno(), struct.pack("=Q", 999), 8)
                wrong = subprocess.run([str(peer_binary), "inspect", str(basic)], capture_output=True, text=True)
                assert wrong.returncode != 0 and "incompatible ledger" in wrong.stderr
            finally:
                os.pwrite(file.fileno(), original, 8)
        report["layout_version_refused"] = True
        report["passed"] = True
finally:
    if fork_directory is not None and fork_directory.exists():
        (fork_directory / "child-stop").touch()
    if fork_pidfd is not None:
        try:
            signal.pidfd_send_signal(fork_pidfd, signal.SIGKILL)
        except ProcessLookupError:
            pass
        os.close(fork_pidfd)
    for actor in actors:
        if actor.process.poll() is None:
            actor.process.kill()
            actor.process.wait(timeout=15)
        actor.stderr.close()
        actor.selector.close()
    (args.output / "events.json").write_text(json.dumps(events, indent=2) + "\n")
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report, indent=2))
