"""Orbit: one tool, which turns the camera around a picture by calling minimax-h3's `render_shot` with MATLOWAI's ORB360
LoRA. That call hands the LoRA over as a model choice, which TensorD binds from the Hub, as minimax-h3's `edit` does with
its edit LoRA, so its cell is skipped as that one is; the package still builds, imports, type-checks and describes on the
candidate Runtime here. A turn runs in the release gate (se-116)."""

from __future__ import annotations

from pathlib import Path

from cell import Cell


def models(root: Path, store: Path) -> dict[str, Path | str]:
    return {}


CELLS: list[Cell] = [
    {"entry": "orbit", "name": "turn", "payload": {},
     "skip": "orbit calls minimax-h3's render_shot with its ORB360 LoRA as a model choice, which TensorD binds from the "
             "Hub (as minimax-h3's edit)"},
]
