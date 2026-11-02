"""Run the frozen existing real mixed-retirement fixture in its own process."""

import runpy
import sys
from pathlib import Path

binaries = Path(__file__).resolve().parent / "bin"
sys.path.insert(0, str(binaries))
sys.argv = [
    str(binaries / "retirement_gpu.py"),
    "--rust",
    str(binaries / "mixed-rust.so"),
]
runpy.run_path(sys.argv[0], run_name="__main__")
