"""Real stopped OFD-publication holder and native nonblocking owner lifecycle; no CUDA.

Run in a task-owned/private tmpfs. Published bytes are fixture metadata, not GPU release.
The independent holder uses the actual Linux OFD publication range; it does not fake a lock
or replace native publication. Interrupted-bank mutation remains covered by prove.py.
"""
from pathlib import Path
import argparse
import ctypes
import fcntl
import hashlib
import json
import os
import selectors
import signal
import subprocess
import sys
import tempfile
import time


class Flock(ctypes.Structure):
    _fields_ = [("kind", ctypes.c_short), ("whence", ctypes.c_short),
                ("start", ctypes.c_longlong), ("length", ctypes.c_longlong), ("pid", ctypes.c_int)]


def lock(fd, kind, start=0):
    fcntl.fcntl(fd, fcntl.F_OFD_SETLK, bytes(Flock(kind, os.SEEK_SET, start, 1, 0)))


if len(sys.argv) > 1 and sys.argv[1] == "--holder":
    fd = os.open(sys.argv[2], os.O_RDWR | os.O_NOFOLLOW)
    lock(fd, fcntl.F_WRLCK)
    print(json.dumps({"held": True, "pid": os.getpid()}), flush=True)
    assert input() == "release"
    lock(fd, fcntl.F_UNLCK)
    print(json.dumps({"released": True}), flush=True)
    assert input() == "exit"
    os.close(fd)
    raise SystemExit

parser = argparse.ArgumentParser()
parser.add_argument("peer", type=Path)
parser.add_argument("output", type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
events, actors = [], []
holder = None


class Peer:
    def __init__(self, name, directory):
        self.name = name
        self.error = (args.output / f"{name}.stderr").open("w")
        self.process = subprocess.Popen([str(args.peer.resolve()), "nonblocking", str(directory)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.error)
        self.buffer = b""
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        actors.append(self)
        self.ready = self.read()

    def read(self):
        deadline = time.monotonic() + 3
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            assert remaining > 0 and self.selector.select(remaining), f"{self.name}: native operation blocked; protocol deadline is not a death inference"
            data = os.read(self.process.stdout.fileno(), 65536)
            assert data, (self.name, self.process.poll())
            self.buffer += data
        line, self.buffer = self.buffer.split(b"\n", 1)
        value = json.loads(line)
        events.append({"peer": self.name, "value": value})
        return value

    def call(self, command):
        self.process.stdin.write((command + "\n").encode()); self.process.stdin.flush()
        return self.read()


report = {"passed": False, "cuda_initialized": False,
          "scope": "real native metadata/OFD/process proof only; no driver capacity measurement",
          "binary_sha256": hashlib.sha256(args.peer.read_bytes()).hexdigest()}
try:
    with tempfile.TemporaryDirectory(prefix="varena-nonblocking-", dir="/dev/shm") as directory:
        owner, observer = Peer("owner", directory), Peer("observer", directory)
        assert owner.call("publish 0 64 0 64 1")["ok"]
        sample = observer.call("sample 10000 1000 1")["sample"]
        round_id = observer.call(f"ask {sample} 64")["round"]
        path = Path(owner.ready["path"])
        assert path.resolve().is_relative_to(Path(directory).resolve()) and not path.is_symlink()
        holder = subprocess.Popen([sys.executable, __file__, "--holder", str(path)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        assert json.loads(holder.stdout.readline())["held"]
        os.kill(holder.pid, signal.SIGSTOP)
        assert os.WIFSTOPPED(os.waitpid(holder.pid, os.WUNTRACED)[1])
        report["stopped_holder"] = holder.pid
        for actor, command in [(observer, "snapshot"), (owner, "publish 0 32 0 32 2"),
                               (observer, "sample 10000 1000 2"), (observer, f"ask {sample} 64")]:
            answer = actor.call(command)
            assert answer.get("error") == "ledger publication is busy", answer
        opening = subprocess.run([str(args.peer.resolve()), "nonblocking", directory],
            input="", capture_output=True, text=True, timeout=3)
        assert opening.returncode != 0 and "ledger publication is busy" in opening.stderr, opening
        report["startup_and_operations_deferred"] = True
        assert owner.call("leave")["left"]
        alive = owner.call("alive")
        assert alive["pid"] == owner.process.pid and not alive["participant"]
        assert owner.process.poll() is None
        with path.open("r+b", buffering=0) as file:
            # The independent row FD was released although this owner PID remains alive.
            lock(file.fileno(), fcntl.F_WRLCK, owner.ready["slot"] + 1)
            lock(file.fileno(), fcntl.F_UNLCK, owner.ready["slot"] + 1)
        report["close_returned_with_live_pid_and_free_row_lock"] = alive

        os.kill(holder.pid, signal.SIGCONT)
        holder.stdin.write("release\n"); holder.stdin.flush()
        assert json.loads(holder.stdout.readline())["released"]
        assert holder.poll() is None, "unlock was not inferred from holder death"
        stale = observer.call("snapshot")
        assert any(row["pid"] == owner.process.pid for row in stale["rows"])
        assert stale["round"]["id"] == round_id and stale["round"]["completed"] == 0
        # Actual coordinator order: separately commit reaping, then take/commit an observation.
        fresh = observer.call("sample 10000 1000 3")
        after = observer.call("snapshot")
        assert all(row["pid"] != owner.process.pid for row in after["rows"])
        assert after["sample"]["used"] == 1000 and after["sample"]["id"] == fresh["sample"]
        assert after["round"]["completed"] == 0 and after["round"]["finished"]
        assert after["round"]["donors"][0]["answer"] == "OwnerGone"
        assert owner.call("alive")["pid"] == owner.process.pid
        report["resume_reap_fresh_sample"] = {"before": stale, "after": after,
            "departed_pid_still_alive": True, "row_disappearance_credited_zero": True}
        holder.stdin.write("exit\n"); holder.stdin.flush(); holder.wait(timeout=3)
        holder = None
        for actor in actors:
            actor.process.stdin.write(b"close\n"); actor.process.stdin.flush()
            actor.process.wait(timeout=3)
            assert actor.process.returncode == 0
        report["passed"] = True
except BaseException as error:
    report["error"] = f"{type(error).__name__}: {error}"
    raise
finally:
    if holder is not None:
        if holder.poll() is None:
            os.kill(holder.pid, signal.SIGCONT)
            holder.kill(); holder.wait(timeout=3)
    for actor in actors:
        if actor.process.poll() is None:
            actor.process.kill(); actor.process.wait(timeout=3)
        actor.error.close(); actor.selector.close()
    (args.output / "events.json").write_text(json.dumps(events, indent=2) + "\n")
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report, indent=2))
