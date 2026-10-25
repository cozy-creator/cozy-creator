"""Build a gate package around the exact production function, without copying its logic."""

import argparse
import hashlib
import json
import shutil
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "sdxl", type=Path, help="candidate packages worktree's sdxl directory"
    )
    parser.add_argument(
        "output", type=Path, help="new durable build directory; must not exist"
    )
    args = parser.parse_args()
    source = args.sdxl.resolve() / "sdxl" / "_prepared.py"
    content = source.read_bytes()
    commit = subprocess.check_output(
        ["git", "-C", str(args.sdxl), "rev-parse", "HEAD"], text=True
    ).strip()
    target = args.output.resolve()
    if target.exists():
        raise SystemExit("output already exists; choose a new build directory")
    shutil.copytree(Path(__file__).parent, target)
    package = target / "ci_sdxl_prepared"
    (package / "_candidate.py").write_bytes(content)
    (package / "source.json").write_text(
        json.dumps(
            {
                "package_commit": commit,
                "source_sha256": hashlib.sha256(content).hexdigest(),
            }
        )
        + "\n"
    )
    print(target)


if __name__ == "__main__":
    main()
