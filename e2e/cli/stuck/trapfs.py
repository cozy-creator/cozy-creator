"""A loopback filesystem that stops answering when told (cl-977), for e2e/cli/stuck.sh.

  trapfs.py <backing dir> <mount point> <trap file>

The trap file says what goes unanswered until it is emptied: a pid, every FLUSH from that process's threads (its exit closes its
files there, so its last thread sits in the kernel, D state, as muromisan's TensorD sat in the NVIDIA driver); `writes`, every
WRITE (a process logging there hangs as on a network filesystem that stopped). Everything else is answered at once.
"""
import errno
import os
import sys
import time

from fuse import FUSE, FuseOSError, Operations, fuse_get_context


class Trap(Operations):
    def __init__(self, root, trap):
        self.root, self.trap = root, trap

    def _path(self, path):
        return os.path.join(self.root, path.lstrip("/"))

    def _said(self):
        try:
            with open(self.trap) as file:
                return file.read().strip()
        except OSError:
            return ""

    def _of(self, tid, pid):
        try:
            with open(f"/proc/{tid}/status") as status:
                return any(line.split()[1:2] == [pid] for line in status if line.startswith("Tgid:"))
        except OSError:
            return False

    def _hold(self, what):
        _, _, tid = fuse_get_context()
        held = False
        while True:
            said = self._said()
            if not (said == what or (what == "flush" and said.isdigit() and self._of(tid, said))):
                break
            if not held:
                print(f"holding a {what} from {tid}", flush=True)
                held = True
            time.sleep(0.2)
        if held:
            print(f"answered a {what} from {tid}", flush=True)

    def getattr(self, path, fh=None):
        try:
            st = os.lstat(self._path(path))
        except OSError as error:
            raise FuseOSError(error.errno)
        keys = ("st_atime", "st_ctime", "st_gid", "st_mode", "st_mtime", "st_nlink", "st_size", "st_uid")
        return {key: getattr(st, key) for key in keys}

    def readdir(self, path, fh):
        return [".", ".."] + os.listdir(self._path(path))

    def access(self, path, mode):
        if not os.access(self._path(path), mode):
            raise FuseOSError(errno.EACCES)

    def open(self, path, flags):
        return os.open(self._path(path), flags)

    def create(self, path, mode, fi=None):
        return os.open(self._path(path), os.O_WRONLY | os.O_CREAT, mode)

    def read(self, path, size, offset, fh):
        return os.pread(fh, size, offset)

    def write(self, path, data, offset, fh):
        self._hold("writes")
        return os.pwrite(fh, data, offset)

    def truncate(self, path, length, fh=None):
        os.truncate(self._path(path), length)

    def chmod(self, path, mode):
        os.chmod(self._path(path), mode)

    def chown(self, path, uid, gid):
        os.chown(self._path(path), uid, gid)

    def utimens(self, path, times=None):
        os.utime(self._path(path), times)

    def unlink(self, path):
        os.unlink(self._path(path))

    def flush(self, path, fh):
        self._hold("flush")
        return 0

    def fsync(self, path, datasync, fh):
        os.fsync(fh)
        return 0

    def release(self, path, fh):
        os.close(fh)
        return 0


if __name__ == "__main__":
    backing, mount, trap = sys.argv[1:4]
    FUSE(Trap(backing, trap), mount, foreground=True, nothreads=False)
