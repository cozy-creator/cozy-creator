# /// script
# requires-python = ">=3.12"
# ///
"""cl-775 probe: can code with an executor's privileges read another process's memory on this pod?

Benign only. It reads four bytes at the start of the target's own executable mapping and checks them
against the ELF magic it already read from that file on disk. It never scans memory, never reads any
other region, and prints no byte it did not already know. Output is a single JSON object on stdout.

  python3 mem.py <target-pid>
"""

import ctypes
import ctypes.util
import json
import os
import sys

ELF_MAGIC = b"\x7fELF"


def facts(pid: int) -> dict:
    out: dict = {"target_pid": pid}
    out["self_euid"] = os.geteuid()
    out["self_caps_effective"] = read_status_field("/proc/self/status", "CapEff")
    out["self_caps_readable"] = capsh()
    out["ptrace_scope"] = read_text("/proc/sys/kernel/yama/ptrace_scope")
    try:
        st = os.stat(f"/proc/{pid}")
        out["target_proc_owner_uid"] = st.st_uid
        out["target_proc_owner_gid"] = st.st_gid
    except OSError as e:
        out["target_proc_owner_uid"] = f"errno {e.errno}"
    # Ownership and mode of /proc/<pid>/mem is the outside proxy for the target's dumpable state:
    # a process made non-dumpable has these owned by root and locked down. A true PR_GET_DUMPABLE
    # can only be read from inside the target, which is noted in the report.
    try:
        ms = os.stat(f"/proc/{pid}/mem")
        out["target_mem_owner_uid"] = ms.st_uid
        out["target_mem_mode"] = oct(ms.st_mode & 0o777)
    except OSError as e:
        out["target_mem_stat"] = f"errno {e.errno}"
    out["target_comm"] = read_text(f"/proc/{pid}/comm")
    return out


def read_text(path: str) -> str:
    try:
        with open(path) as f:
            return f.read().strip()
    except OSError as e:
        return f"errno {e.errno}"


def read_status_field(path: str, field: str) -> str:
    try:
        with open(path) as f:
            for line in f:
                if line.startswith(field + ":"):
                    return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return ""


def capsh() -> str:
    # A human-readable capability set, when capsh is present; never required.
    import shutil
    import subprocess

    tool = shutil.which("capsh")
    if not tool:
        return "(no capsh)"
    try:
        res = subprocess.run([tool, "--print"], capture_output=True, text=True, timeout=10)
        for line in res.stdout.splitlines():
            if line.startswith("Current:"):
                return line.strip()
    except Exception as e:  # noqa: BLE001
        return f"(capsh failed: {e})"
    return "(capsh gave no Current line)"


def exe_mapping(pid: int) -> tuple[int, str] | None:
    """The first readable, executable mapping backed by the target's own executable, and its path."""
    try:
        target_exe = os.readlink(f"/proc/{pid}/exe")
    except OSError:
        target_exe = ""
    try:
        with open(f"/proc/{pid}/maps") as f:
            for line in f:
                parts = line.split()
                if len(parts) < 6:
                    continue
                addrs, perms, path = parts[0], parts[1], parts[5]
                if "r" in perms and "x" in perms and path == target_exe:
                    start = int(addrs.split("-")[0], 16)
                    return start, path
    except OSError:
        return None
    return None


def disk_magic(path: str) -> bytes:
    try:
        with open(path, "rb") as f:
            return f.read(4)
    except OSError:
        return b""


def via_proc_mem(pid: int, addr: int) -> dict:
    try:
        fd = os.open(f"/proc/{pid}/mem", os.O_RDONLY)
    except OSError as e:
        return {"opened": False, "errno": e.errno, "strerror": os.strerror(e.errno)}
    try:
        os.lseek(fd, addr, os.SEEK_SET)
        data = os.read(fd, 4)
        return {"opened": True, "read_ok": data == ELF_MAGIC, "bytes_are_elf_magic": data == ELF_MAGIC}
    except OSError as e:
        return {"opened": True, "read_ok": False, "errno": e.errno, "strerror": os.strerror(e.errno)}
    finally:
        os.close(fd)


def via_vm_readv(pid: int, addr: int) -> dict:
    libc = ctypes.CDLL(ctypes.util.find_library("c") or "libc.so.6", use_errno=True)

    class IoVec(ctypes.Structure):
        _fields_ = [("base", ctypes.c_void_p), ("len", ctypes.c_size_t)]

    buf = (ctypes.c_char * 4)()
    local = IoVec(ctypes.cast(buf, ctypes.c_void_p), 4)
    remote = IoVec(ctypes.c_void_p(addr), 4)
    ctypes.set_errno(0)
    n = libc.process_vm_readv(
        ctypes.c_int(pid), ctypes.byref(local), ctypes.c_ulong(1),
        ctypes.byref(remote), ctypes.c_ulong(1), ctypes.c_ulong(0),
    )
    if n < 0:
        err = ctypes.get_errno()
        return {"call_ok": False, "errno": err, "strerror": os.strerror(err)}
    return {"call_ok": True, "bytes_read": n, "bytes_are_elf_magic": bytes(buf)[:n] == ELF_MAGIC[:n]}


def main() -> None:
    pid = int(sys.argv[1])
    out = facts(pid)
    mapping = exe_mapping(pid)
    if mapping is None:
        out["mapping"] = "no readable executable mapping found in the target's maps"
        print(json.dumps(out))
        return
    addr, path = mapping
    out["target_exe"] = path
    out["disk_first_bytes_are_elf"] = disk_magic(path) == ELF_MAGIC
    out["proc_mem"] = via_proc_mem(pid, addr)
    out["process_vm_readv"] = via_vm_readv(pid, addr)
    readable = bool(out["proc_mem"].get("read_ok")) or bool(out["process_vm_readv"].get("bytes_are_elf_magic"))
    out["memory_readable"] = readable
    print(json.dumps(out))


if __name__ == "__main__":
    main()
