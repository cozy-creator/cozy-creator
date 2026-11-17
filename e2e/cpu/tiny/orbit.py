"""Orbit: one tool, which turns the camera around a picture by calling minimax-h3's `render_shot` with MATLOWAI's ORB360
LoRA. That call is to another package, which this driver's child executor (the calling package's own functions) does
not run, so its cell is skipped; the package still builds, imports, type-checks and describes on the candidate Runtime
here. A turn runs in the release gate (se-116)."""

from __future__ import annotations

from pathlib import Path

from cell import Cell


def models(root: Path, store: Path) -> dict[str, Path | str]:
    return {}


CELLS: list[Cell] = [
    {"entry": "orbit", "name": "turn", "payload": {},
     "skip": "orbit calls minimax-h3's render_shot, another package's function, which this driver does not run"},
]
