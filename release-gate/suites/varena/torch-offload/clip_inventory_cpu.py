"""Real tiny stock CLIP CPU inventory regression; no downloads or GPU proof."""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path

import torch
import transformers
from transformers import CLIPTextConfig, CLIPTextModel, CLIPTextModelWithProjection

ROOT = Path(__file__).resolve().parents[4]
SOURCE = ROOT / "crates/tensorfs-py/python/tensorfs/varena/_offload_inventory.py"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--inventory-source", type=Path, default=SOURCE)
SOURCE = parser.parse_args().inventory_source
spec = importlib.util.spec_from_file_location("clip_inventory_under_review", SOURCE)
inventory = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = inventory
spec.loader.exec_module(inventory)


def main():
    assert not torch.cuda.is_initialized()
    torch.set_num_threads(1)
    torch.manual_seed(17)
    config = CLIPTextConfig(
        vocab_size=16,
        hidden_size=8,
        intermediate_size=16,
        num_hidden_layers=2,
        num_attention_heads=2,
        max_position_embeddings=8,
        projection_dim=8,
        bos_token_id=0,
        eos_token_id=3,
        pad_token_id=1,
    )
    rows = []
    for cls in (CLIPTextModel, CLIPTextModelWithProjection):
        model = cls(config).eval()
        declaration = model._no_split_modules
        row = {
            "class": cls.__name__,
            "declaration_type": type(declaration).__name__,
            "declaration": sorted(declaration),
        }
        before = tuple(
            (id(p), p.data_ptr(), p.requires_grad) for p in model.parameters()
        )
        inputs = torch.tensor([[1, 2, 3, 4]])
        with torch.no_grad():
            expected = model(inputs).to_tuple()
        try:
            result = inventory.inspect_cpu_roots({"text_encoder": model})
        except inventory.InventoryError as error:
            row["error"] = str(error)
        else:
            expected_boundaries = tuple(
                f"text_encoder.{name}"
                for name, module in model.named_modules()
                if type(module).__name__ in {"CLIPTextEmbeddings", "CLIPEncoderLayer"}
            )
            assert len(expected_boundaries) == 3
            assert result.boundaries == expected_boundaries, result.boundaries
            assert result.boundaries == inventory.inspect_cpu_roots(
                {"text_encoder": model}
            ).boundaries
            assert tuple(
                (id(p), p.data_ptr(), p.requires_grad) for p in model.parameters()
            ) == before
            assert model._no_split_modules is declaration
            with torch.no_grad():
                actual = model(inputs).to_tuple()
            torch.testing.assert_close(actual, expected, rtol=0, atol=0)
            row["boundaries"] = result.boundaries
            row["preserves_output_identity_storage_and_flags"] = True
        rows.append(row)
    assert not torch.cuda.is_initialized()
    print(
        json.dumps(
            {
                "torch": torch.__version__,
                "torch_cuda_build": torch.version.cuda,
                "transformers": transformers.__version__,
                "models": rows,
                "cuda": False,
                "model_downloads": False,
            },
            sort_keys=True,
        )
    )
    assert all("error" not in row for row in rows), rows


if __name__ == "__main__":
    main()
