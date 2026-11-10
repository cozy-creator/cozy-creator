"""MiniMax H3's tools: weights jobs over an H3 checkpoint. The tools' environment holds torch and TensorFS alone, so the
tiny H3 (`tiny/minimax_h3.py`, built from minimax-h3's own classes) comes as its layout (`minimax_h3_layout.json`: each
component's config, tensors' names, dtypes and shapes, and assets, from that fixture), filled from a seed here.

`vae` and the `bf16` lane run; the encoded lanes' reviewed plan pins the release's shapes, the lanes that rewrite AdaLN
rows (`bf16-full`, `bf16-pruned`, `fp8-pruned`, the table jobs) take a full (unpruned) H3, and the LoRA jobs take upstream
adapters: those are listed (SKIP)."""

from __future__ import annotations

import base64
import json
from pathlib import Path

import tensorfs
import torch

from cell import Cell, Json, quantize

H3 = "tiny/minimax-h3"
_DTYPES = {"BF16": torch.bfloat16, "F32": torch.float32, "F16": torch.float16}
_FULL = "takes a full (unpruned) H3 release or upstream adapters; the tiny H3 is AdaLN-pruned"


def models(root: Path, store: Path) -> dict[str, Path | str]:
    layout: dict[str, dict[str, Json]] = json.loads((Path(__file__).parent / "minimax_h3_layout.json").read_text())
    generator = torch.Generator().manual_seed(0)
    writer = tensorfs.Writer(cache_dir=store)
    for name, held in layout.items():
        config = held["config"]
        assert isinstance(config, dict)
        component = writer.component(name, cls=None, config=config)
        tensors, assets = held["tensors"], held["assets"]
        assert isinstance(tensors, list) and isinstance(assets, dict)
        for row in tensors:
            assert isinstance(row, list)
            key, dtype, shape = str(row[0]), str(row[1]), [int(str(n)) for n in row[2]] if isinstance(row[2], list) else []
            data = (torch.randn(shape, generator=generator) * 0.02).to(_DTYPES[dtype]).contiguous()
            component.put(key, memoryview(data.view(torch.uint8).numpy()).cast("B"), dtype=dtype, shape=shape)
        for asset, body in assets.items():
            component.asset(asset, base64.b64decode(str(body)))
    return {H3: writer.commit().id}


_SKIPPED: list[Cell] = [{"entry": job, "name": "lane", "payload": {}, "skip": _FULL} for job in (
    "bf16-full", "bf16-pruned", "fp8-pruned", "curve-pruned", "retable", "select-adaln-weights", "compute-adaln-tables",
    "apply-adaln", "retable-adaln", "turbo-lora", "regenerate-lora")]

CELLS: list[Cell] = [
    {"entry": "vae", "name": "lane", "payload": {}, "sources": {"source": H3}, "weights": {"vae": 1 << 34}},
    quantize(H3, "bf16", "minimax-h3-tools", "minimax-h3-tools@0.3.2"),
    {**quantize(H3, "fp8", "minimax-h3-tools", "minimax-h3-tools@0.3.2"),
     "skip": "the encoded lanes' reviewed DiT plan pins the release's shapes (context_embedder [5376, 5120])"},
    *_SKIPPED,
]
