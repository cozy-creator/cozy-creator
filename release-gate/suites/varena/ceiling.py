# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19"]
# ///
"""This machine's load ceiling, measured on it (hardware-ceiling-bars): a cold sequential read of a fresh 2 GiB file on
the disk the machine's software lives on. The varena suite judges every load from the store as a share of it. The
host-to-device ceiling is unmeasured."""
import json
import os
import sys
import tempfile
import time

import msgspec

from cozy_runtime.author import App

app = App()


class Nothing(msgspec.Struct):
    pass


class Measured(msgspec.Struct):
    measured: str

GIB = 1 << 30


def disk(total: int = 2 * GIB, chunk: int = 16 << 20) -> float:
    block = os.urandom(chunk)
    fd, path = tempfile.mkstemp(dir=sys.prefix)
    try:
        for _ in range(total // chunk):
            os.write(fd, block)
        os.fsync(fd)
        os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        os.lseek(fd, 0, os.SEEK_SET)
        start = time.perf_counter()
        while os.read(fd, chunk):
            pass
        return total / (time.perf_counter() - start) / 1e9
    finally:
        os.close(fd)
        os.unlink(path)


@app.entrypoint
def main(payload: Nothing) -> Measured:
    return Measured(json.dumps({"disk_gbps": round(disk(), 2)}))
