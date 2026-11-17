"""e2e-slow-install's dependency (cl-1104): its wheel, made the same each time (its lock pins its hash), served from 127.0.0.1:47104
at `bytes_per_s` (0: as fast as it goes), as prefetch_disk needs a guess's environment build that takes a while.
    wheel.py <dir> [<bytes_per_s>]: writes the wheel into <dir>, then serves <dir> until killed."""

import base64
import hashlib
import http.server
import random
import sys
import time
import zipfile
from pathlib import Path

NAME, PORT, SIZE = "e2e_slow_wheel", 47104, 96 << 20


def wheel(directory: Path) -> Path:
    data = random.Random(1104).randbytes(SIZE)
    info = f"{NAME}-0.0.0.dist-info"
    files = {f"{NAME}/__init__.py": b"", f"{NAME}/data.bin": data,
             f"{info}/METADATA": b"Metadata-Version: 2.1\nName: e2e-slow-wheel\nVersion: 0.0.0\n",
             f"{info}/WHEEL": b"Wheel-Version: 1.0\nGenerator: e2e\nRoot-Is-Purelib: true\nTag: py3-none-any\n"}

    def digest(content: bytes) -> str:
        return "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(content).digest()).rstrip(b"=").decode()

    record = "".join(f"{path},{digest(content)},{len(content)}\n" for path, content in files.items()) + f"{info}/RECORD,,\n"
    path = directory / f"{NAME}-0.0.0-py3-none-any.whl"
    with zipfile.ZipFile(path, "w", zipfile.ZIP_STORED) as out:
        for name, content in [*files.items(), (f"{info}/RECORD", record.encode())]:
            out.writestr(zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0)), content)
    return path


def main() -> None:
    directory = Path(sys.argv[1])
    rate = int(sys.argv[2]) if len(sys.argv) > 2 else 0
    directory.mkdir(parents=True, exist_ok=True)
    wheel(directory)

    class Slow(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args: object, **kwargs: object) -> None:
            super().__init__(*args, directory=str(directory), **kwargs)  # type: ignore[arg-type]

        def copyfile(self, source, outputfile) -> None:  # type: ignore[no-untyped-def]
            chunk = 1 << 16
            while block := source.read(chunk):
                outputfile.write(block)
                if rate:
                    time.sleep(len(block) / rate)

    http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Slow).serve_forever()


if __name__ == "__main__":
    main()
