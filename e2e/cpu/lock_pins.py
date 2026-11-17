"""Whether a uv.lock pins each named package at that version from PyPI.

    lock_pins.py <uv.lock> <name>==<version>...

Prints what it pins ("cozy-runtime 0.47.0, tensorfs 0.26.0 from PyPI") and exits 0, or the first that differs and exits 1.
"""

from __future__ import annotations

import sys
import tomllib

PYPI = "https://pypi.org/simple"


def main() -> int:
    with open(sys.argv[1], "rb") as file:
        packages = tomllib.load(file).get("package", [])
    said = []
    for want in sys.argv[2:]:
        name, version = want.split("==")
        pins = [(p["version"], p.get("source", {}).get("registry") or p.get("source")) for p in packages if p["name"] == name]
        if not pins or any(pin != (version, PYPI) for pin in pins):
            print(f"{name}: the lock pins {', '.join(f'{v} from {s}' for v, s in pins) or 'nothing'}, not PyPI's {version}")
            return 1
        said.append(f"{name} {version}")
    print(", ".join(said) + " from PyPI")
    return 0


if __name__ == "__main__":
    sys.exit(main())
