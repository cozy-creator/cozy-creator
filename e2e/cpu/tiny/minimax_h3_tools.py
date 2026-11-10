"""MiniMax H3's tools: weights jobs over a converted H3 release (the lanes, AdaLN tables, VAE, LoRAs). A tiny H3 is built
from minimax-h3's own classes (`tiny/minimax_h3.py`), which the tools' environment (torch and TensorFS alone) does not
hold, so the jobs are listed here and the environment, typecheck and conformance cells run."""

from __future__ import annotations

from pathlib import Path

from cell import Cell

_WHY = "takes a converted H3 checkpoint; the tiny one needs minimax-h3's classes, which this environment does not hold"


def models(root: Path, store: Path) -> dict[str, Path | str]:
    return {}


CELLS: list[Cell] = [
    {"entry": job, "name": "lane", "payload": {}, "skip": _WHY}
    for job in ("bf16-full", "bf16-pruned", "fp8-pruned", "curve-pruned", "quantize", "vae", "retable",
                "select-adaln-weights", "compute-adaln-tables", "apply-adaln", "retable-adaln", "turbo-lora",
                "regenerate-lora")
]
