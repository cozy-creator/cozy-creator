"""Whether the uv.lock Calcifer made on this suite's Hub leaves the committed one as it is.

    lock_same.py <committed uv.lock> <made uv.lock> [<the suite Hub's origin>]

A lock that takes a package from a Hub index (orbit's minimax-h3) names production's (`https://tensorhub.com/v1/index/`).
Made on the suite's own Hub it names that Hub's origin instead, and each of that index's wheels is that Hub's build of the
same release: those URLs are read at production's origin and those wheels' hashes are not compared. Every other byte must
be the committed lock's. Prints the first line that differs and exits 1, or exits 0.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

PRODUCTION = "https://tensorhub.com"
#: A Hub index's file and its hash, as uv writes it: `{ url = "<hub>/v1/index/...", hash = "sha256:..." }`.
HUB_FILE = re.compile(r'(url = "' + re.escape(PRODUCTION) + r'/v1/index/[^"]*", hash = )"sha256:[0-9a-f]{64}"')


def normal(text: str, origin: str) -> str:
    if origin:
        text = text.replace(origin.rstrip("/") + "/v1/index/", PRODUCTION + "/v1/index/")
    return HUB_FILE.sub(r'\1"sha256:*"', text)


def main() -> int:
    committed, made = (Path(arg).read_text() for arg in sys.argv[1:3])
    origin = sys.argv[3] if len(sys.argv) > 3 else ""
    want, got = normal(committed, origin), normal(made, origin)
    if want == got:
        return 0
    lines = list(zip(want.splitlines(), got.splitlines()))
    number = next((i for i, (a, b) in enumerate(lines, 1) if a != b), len(lines) + 1)
    a, b = (text.splitlines()[number - 1] if number <= len(text.splitlines()) else "(end)" for text in (want, got))
    print(f"line {number}: {a.strip()} -> {b.strip()}"[:300])
    return 1


if __name__ == "__main__":
    sys.exit(main())
