"""A loopback filesystem that stops answering when told (cl-977), for e2e/cli/stuck.sh.

  trapfs.py <backing dir> <mount point> <trap file>

The trap file says what goes unanswered until it is emptied: pids, every FLUSH from those processes' threads (an exit closes its
files there, so its last thread sits in the kernel, as muromisan's TensorD sat in the NVIDIA driver); `<pid>:<seconds>`, the same for
that long (a slow exit); `writes`, every WRITE (a process logging there hangs as on a network filesystem that stopped). Everything
else is answered at once. No request is held past MAX_HOLD, so a cell that dies midway leaves nothing waiting for good.
"""
import errno
import os
import sys
import time

from fuse import FUSE, FuseOSError, Operations, fuse_get_context

MAX_HOLD = 600


class Trap(Operations):
    def __init__(self, root: str, trap: str) -> None:
        self.root, self.trap = root, trap

    def _path(self, path: str) -> str:
        return os.path.join(self.root, path.lstrip("/"))

    def _said(self) -> str:
        try:
            with open(self.trap) as file:
                return file.read().strip()
        except OSError:
            return ""

    def _of(self, tid: int, pid: str) -> bool:
        try:
            with open(f"/proc/{tid}/status") as status:
                return any(line.split()[1:2] == [pid] for line in status if line.startswith("Tgid:"))
        except OSError:
            return False

    def _holds(self, what: str, tid: int) -> float:
        """How long a `what` from thread `tid` is held now, 0 for not at all."""
        said = self._said().split()
        if what == "writes":
            return MAX_HOLD if "writes" in said else 0
        for token in said:
            pid, _, seconds = token.partition(":")
            if pid.isdigit() and self._of(tid, pid):
                return float(seconds) if seconds else MAX_HOLD
        return 0

    def _hold(self, what: str) -> None:
        _, _, tid = fuse_get_context()
        held, began = False, time.monotonic()
        while True:
            if time.monotonic() - began >= min(self._holds(what, tid), MAX_HOLD):
                break
            if not held:
                print(f"holding a {what} from {tid}", flush=True)
                held = True
            time.sleep(0.2)
        if held:
            print(f"answered a {what} from {tid}", flush=True)

    def getattr(self, path: str, fh: int | None = None) -> dict[str, float]:
        try:
            st = os.lstat(self._path(path))
        except OSError as error:
            raise FuseOSError(error.errno or errno.EIO)
        keys = ("st_atime", "st_ctime", "st_gid", "st_mode", "st_mtime", "st_nlink", "st_size", "st_uid")
        return {key: float(getattr(st, key)) for key in keys}

    def readdir(self, path: str, fh: int) -> list[str]:
        return [".", ".."] + os.listdir(self._path(path))

    def access(self, path: str, mode: int) -> None:
        if not os.access(self._path(path), mode):
            raise FuseOSError(errno.EACCES)

    def open(self, path: str, flags: int) -> int:
        return os.open(self._path(path), flags)

    def create(self, path: str, mode: int, fi: object = None) -> int:
        return os.open(self._path(path), os.O_WRONLY | os.O_CREAT, mode)

    def read(self, path: str, size: int, offset: int, fh: int) -> bytes:
        return os.pread(fh, size, offset)

    def write(self, path: str, data: bytes, offset: int, fh: int) -> int:
        self._hold("writes")
        return os.pwrite(fh, data, offset)

    def truncate(self, path: str, length: int, fh: int | None = None) -> None:
        os.truncate(self._path(path), length)

    def chmod(self, path: str, mode: int) -> None:
        os.chmod(self._path(path), mode)

    def chown(self, path: str, uid: int, gid: int) -> None:
        os.chown(self._path(path), uid, gid)

    def utimens(self, path: str, times: tuple[float, float] | None = None) -> None:
        os.utime(self._path(path), times)

    def unlink(self, path: str) -> None:
        os.unlink(self._path(path))

    def flush(self, path: str, fh: int) -> int:
        self._hold("flush")
        return 0

    def fsync(self, path: str, datasync: int, fh: int) -> int:
        os.fsync(fh)
        return 0

    def release(self, path: str, fh: int) -> int:
        os.close(fh)
        return 0


if __name__ == "__main__":
    backing, mount, trap = sys.argv[1:4]
    FUSE(Trap(backing, trap), mount, foreground=True, nothreads=False)
