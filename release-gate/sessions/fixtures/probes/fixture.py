# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38", "msgspec>=0.19"]
# ///
"""The probes session's runs, weightless. `memread` answers cl-775 from inside a real executor; the part above the marker also runs
alone on the standard library, from a machine's own shell. Benign only: at most four bytes of a mapping's start are read, which are
an ELF header, and only whether they are one is reported. Nothing searches memory, and no other process's environment or descriptors
are opened. Of this process's own environment, only whether a name is present is reported, never a value."""

import ctypes
import errno
import json
import os
from typing import TypeAlias, TypedDict, cast

ELF = b"\x7fELF"
PR_GET_DUMPABLE = 3
CAP_SYS_PTRACE = 19
NOBODY = 65534
PROVIDER_NAMES = ("RUNPOD_POD_ID", "RUNPOD_PUBLIC_IP", "CONTAINER_ID", "VAST_CONTAINERLABEL", "PUBLIC_IPADDR")
KEY_NAMES = ("RUNPOD_API_KEY", "CONTAINER_API_KEY")
LIBC = ctypes.CDLL(None, use_errno=True)


class IoVec(ctypes.Structure):
    _fields_ = [("base", ctypes.c_void_p), ("length", ctypes.c_size_t)]


Fact: TypeAlias = str | int | bool | None


class ProcMem(TypedDict, total=False):
    open: str
    read: str
    is_elf_header: bool


class VmRead(TypedDict, total=False):
    read: str
    is_elf_header: bool


class Attempt(TypedDict):
    proc_mem: ProcMem
    process_vm_readv: VmRead


class ProbeReport(TypedDict):
    self: dict[str, Fact]
    ptrace_scope: str
    suid_dumpable: str
    pid_1: str
    provider_names_present: list[str]
    key_names_in_own_environment: list[str]
    control_own_memory: dict[str, str | None | ProcMem | VmRead]
    tensord: list[dict[str, Fact | Attempt | str]]


def named(code: int) -> str:
    return errno.errorcode.get(code, str(code))


def text(path: str) -> str:
    try:
        with open(path) as file:
            return file.read().strip()
    except OSError as error:
        return named(error.errno)


def status(pid: int | str) -> dict[str, Fact]:
    wanted = ("Uid", "Gid", "PPid", "CapEff", "CapPrm", "CapBnd", "Seccomp", "NoNewPrivs")
    with open(f"/proc/{pid}/status") as file:
        rows = dict(line.rstrip("\n").split(":\t", 1) for line in file if ":\t" in line)
    held: dict[str, Fact] = {name: rows.get(name) for name in wanted}
    held["sys_ptrace"] = bool(int(rows.get("CapEff", "0"), 16) >> CAP_SYS_PTRACE & 1)
    return held


def image_start(pid: int | str) -> tuple[int | None, str | None]:
    """Where a process's first file-backed image begins: file offset 0, so its first bytes are an ELF header."""
    try:
        with open(f"/proc/{pid}/maps") as file:
            for line in file:
                fields = line.split()
                if len(fields) >= 6 and fields[1].startswith("r") and int(fields[2], 16) == 0 and fields[5].startswith("/"):
                    return int(fields[0].split("-")[0], 16), None
    except OSError as error:
        return None, named(error.errno)
    return None, "no image"


def by_proc_mem(pid: int, address: int | None) -> ProcMem:
    try:
        descriptor = os.open(f"/proc/{pid}/mem", os.O_RDONLY)
    except OSError as error:
        return {"open": named(error.errno)}
    try:
        if address is None:
            return {"open": "ok", "read": "no address"}
        return {"open": "ok", "read": "ok", "is_elf_header": os.pread(descriptor, len(ELF), address) == ELF}
    except OSError as error:
        return {"open": "ok", "read": named(error.errno)}
    finally:
        os.close(descriptor)


def by_process_vm(pid: int, address: int | None) -> VmRead:
    if address is None:
        return {"read": "no address"}
    buffer = ctypes.create_string_buffer(len(ELF))
    local = IoVec(ctypes.cast(buffer, ctypes.c_void_p), len(ELF))
    remote = IoVec(address, len(ELF))
    call = LIBC.process_vm_readv
    call.restype = ctypes.c_ssize_t
    call.argtypes = [ctypes.c_int, ctypes.POINTER(IoVec), ctypes.c_ulong, ctypes.POINTER(IoVec), ctypes.c_ulong, ctypes.c_ulong]
    got = call(pid, ctypes.byref(local), 1, ctypes.byref(remote), 1, 0)
    if got < 0:
        return {"read": named(ctypes.get_errno())}
    return {"read": "ok", "is_elf_header": buffer.raw[:got] == ELF}


def attempt(pid: int, address: int | None) -> Attempt:
    return {"proc_mem": by_proc_mem(pid, address), "process_vm_readv": by_process_vm(pid, address)}


def as_nobody(pid: int, address: int | None) -> str | Attempt:
    """The same attempt from a child that has become another user: what running executors as another user would meet."""
    if os.geteuid() != 0:
        return "not root: cannot become another user"
    read_end, write_end = os.pipe()
    child = os.fork()
    if child == 0:
        os.close(read_end)
        os.setgroups([])
        os.setgid(NOBODY)
        os.setuid(NOBODY)
        os.write(write_end, json.dumps(attempt(pid, address)).encode())
        os._exit(0)
    os.close(write_end)
    with os.fdopen(read_end) as answer:
        said = answer.read()
    os.waitpid(child, 0)
    return cast(Attempt, json.loads(said))  # the child's attempt()


def ancestors() -> list[int]:
    chain: list[int]
    chain, pid = [], os.getpid()
    while pid > 1:
        pid = int(cast(str, status(pid)["PPid"]))  # every /proc/<pid>/status has it
        chain.append(pid)
    return chain


def probe() -> ProbeReport:
    pids = sorted(int(name) for name in os.listdir("/proc") if name.isdigit())
    tensords = [pid for pid in pids if text(f"/proc/{pid}/comm") == "tensord"]
    above = ancestors()
    start, why = image_start("self")
    report: ProbeReport = {
        "self": {"pid": os.getpid(), "uid": os.getuid(), "euid": os.geteuid(), "comm": text("/proc/self/comm"),
                 "dumpable": LIBC.prctl(PR_GET_DUMPABLE, 0, 0, 0, 0), **status("self")},
        "ptrace_scope": text("/proc/sys/kernel/yama/ptrace_scope"),
        "suid_dumpable": text("/proc/sys/fs/suid_dumpable"),
        "pid_1": text("/proc/1/comm"),
        "provider_names_present": [name for name in PROVIDER_NAMES if name in os.environ],
        "key_names_in_own_environment": [name for name in KEY_NAMES if name in os.environ],
        "control_own_memory": {"maps": why, **attempt(os.getpid(), start)},
        "tensord": [],
    }
    for pid in tensords:
        start, why = image_start(pid)
        stat = os.stat(f"/proc/{pid}/mem")
        report["tensord"].append({"pid": pid, "is_ancestor": pid in above, **status(pid), "mem_owner_uid": stat.st_uid,
                                  "maps": why or "ok", "attempt": attempt(pid, start), "as_nobody": as_nobody(pid, start)})
    return report


if __name__ == "__main__":
    print(json.dumps(probe()))
# end of the probe

import msgspec  # noqa: E402
from cozy_runtime.author import App, Outputs
from cozy_runtime.media import File  # noqa: E402

app = App()


class Note(msgspec.Struct, kw_only=True):
    note: str = ""


class Report(msgspec.Struct):
    report: File


@app.entrypoint(accelerator=False)
def memread(payload: Note, out: Outputs) -> Report:
    return Report(out.save_bytes(json.dumps(probe()).encode(), media_type="application/json"))


class Size(msgspec.Struct, kw_only=True):
    megabytes: int


class Blob(msgspec.Struct):
    blob: File


@app.entrypoint(accelerator=False)
def blob(payload: Size, out: Outputs) -> Blob:
    return Blob(out.save_bytes(os.urandom(payload.megabytes * 10**6), media_type="application/octet-stream"))
