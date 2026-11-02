"""Real tiny Diffusers CPU inference and VAE dtype-cycle inspection, no offload/GPU proof."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import diffusers
import torch
from diffusers import AutoencoderKL, DiffusionPipeline
from diffusers.hooks import HookRegistry
from diffusers.hooks.group_offloading import apply_group_offloading
from diffusers.hooks.layerwise_casting import apply_layerwise_casting

ROOT = Path(__file__).resolve().parents[4]
spec = importlib.util.spec_from_file_location(
    "offload_inventory_under_review",
    ROOT / "crates/tensorfs-py/python/tensorfs/varena/_offload_inventory.py",
)
inventory = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = inventory
spec.loader.exec_module(inventory)


class CpuPipeline(DiffusionPipeline):
    def __init__(self, vae):
        super().__init__()
        self.register_modules(vae=vae)


def main():
    assert torch.version.cuda is None and not torch.cuda.is_initialized()
    torch.set_num_threads(1)
    torch.manual_seed(13)
    vae = AutoencoderKL(
        in_channels=3,
        out_channels=3,
        down_block_types=("DownEncoderBlock2D",),
        up_block_types=("UpDecoderBlock2D",),
        block_out_channels=(8,),
        layers_per_block=1,
        latent_channels=4,
        norm_num_groups=4,
        sample_size=8,
        force_upcast=True,
    ).eval()
    pipeline = CpuPipeline(vae)
    roots = {
        name: module
        for name, module in pipeline.components.items()
        if isinstance(module, torch.nn.Module)
    }
    original = tuple(id(parameter) for parameter in vae.parameters())
    image = torch.linspace(-1, 1, 3 * 8 * 8).reshape(1, 3, 8, 8)

    def execute():
        with torch.no_grad():
            # These are direct public methods, not a VAE.forward invocation.
            latent = vae.encode(image).latent_dist.mode()
            return vae.decode(latent).sample

    before = execute()
    description = inventory.inspect_cpu_roots(roots)
    after = execute()
    torch.testing.assert_close(after, before, rtol=0, atol=0)
    assert tuple(id(parameter) for parameter in vae.parameters()) == original

    # Preserve the normal conversion request. This is stock CPU behavior to qualify
    # the inspection contract, not a claim that the future offload adapter handles it.
    vae.to(dtype=torch.float16)
    inventory.inspect_cpu_roots(roots)
    vae.to(dtype=torch.float32)
    widened = execute()
    assert torch.isfinite(widened).all()
    vae.to(dtype=torch.float16)
    assert tuple(id(parameter) for parameter in vae.parameters()) == original
    assert vae.config.force_upcast is True
    guarded = torch.nn.Linear(2, 2).eval()
    apply_layerwise_casting(
        guarded,
        storage_dtype=torch.float16,
        compute_dtype=torch.float32,
        skip_modules_pattern=None,
        skip_modules_classes=None,
    )
    try:
        inventory.inspect_cpu_roots({"cast": guarded})
    except inventory.InventoryError as error:
        assert "Diffusers hooks" in str(error)
    else:
        raise AssertionError("layerwise casting accepted without an adapter")
    grouped = torch.nn.Sequential(torch.nn.Linear(2, 2), torch.nn.Linear(2, 2)).eval()
    apply_group_offloading(
        grouped,
        onload_device=torch.device("cpu"),
        offload_device=torch.device("cpu"),
        offload_type="leaf_level",
        use_stream=False,
    )
    try:
        inventory.inspect_cpu_roots({"group": grouped})
    except inventory.InventoryError as error:
        assert "Diffusers hooks" in str(error)
    else:
        raise AssertionError("a competing group offloader was accepted")
    unhooked = torch.nn.Linear(2, 2).eval()
    HookRegistry.check_if_exists_or_initialize(unhooked)
    inventory.inspect_cpu_roots({"empty_registry": unhooked})
    assert not torch.cuda.is_initialized()
    assert not any(
        "libcuda.so" in line
        for line in Path("/proc/self/maps").read_text().splitlines()
    )
    print(
        json.dumps(
            {
                "torch": torch.__version__,
                "diffusers": diffusers.__version__,
                "facts": description.facts(),
                "output_shape": list(before.shape),
                "inspection_preserves_output_and_identity": True,
                "stock_vae_dtype_cycle": True,
                "force_upcast_unchanged": True,
                "model_downloads": False,
                "offload_implemented": False,
                "cuda": False,
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
