"""One actual package VAE block, backed by a genuine local TensorFS checkpoint."""
from __future__ import annotations

from pathlib import Path

import tensorfs
import torch
from cozy_runtime import fusion
from cozy_runtime.author.kernels import Plan
from cozy_runtime.internal import attention, triton_kernels
from cozy_runtime.internal.residency import Declared, Weights
from diffusers.models.autoencoders.autoencoder_kl_minimax_h3 import (
    MiniMaxH3VideoRotaryPosEmbed,
    MiniMaxH3VideoTransformerBlock,
)
from minimax_h3.vae_tiles import VAE_BLOCK, _BlockRows

PATHS = ('attn.to_q', 'attn.to_k', 'attn.to_v', 'attn.to_out.0', 'ff.net.0.proj', 'ff.net.2')


def make(device, matrix_dtype, *, seed=13849):
    with torch.random.fork_rng(devices=[]), torch.device(device):
        torch.random.default_generator.manual_seed(seed)
        block = MiniMaxH3VideoTransformerBlock(dim=2048, heads=32, dim_head=64)
    block.eval().requires_grad_(False)
    for name in PATHS:
        leaf = block.get_submodule(name)
        leaf.weight.data = leaf.weight.data.to(dtype=matrix_dtype)
        assert leaf.bias.dtype is torch.float32
    if str(device) != 'meta':
        block.scale1.data.fill_(0.2)
        block.scale2.data.fill_(0.15)
    return block


class Loaded:
    def __init__(self, request, directory: Path, matrix_dtype, selected):
        self.request = request
        self.reference = make('cpu', matrix_dtype)
        writer = tensorfs.Writer(directory / 'vae-block.cozytensors')
        component = writer.component('vae')
        self.payload_bytes = 0
        for name, value in self.reference.state_dict().items():
            component.put(name, value.detach())
            self.payload_bytes += value.numel() * value.element_size()
        self.checkpoint = writer.commit()
        self.block = make('meta', matrix_dtype)
        for block in (self.block, self.reference):
            block.attn.processor._attention_backend = selected
            block.attn.processor._parallel_config = None
        on = request.memory.device
        if on.type == 'cuda':
            plan = Plan(on, lambda kernels: triton_kernels.load(kernels, device=on))
            assert fusion.install(plan, VAE_BLOCK, self.block, self.reference)
            assert self.block._cozy_dit.switch.launch is self.reference._cozy_dit.switch.launch
        self.block._cozy_weights = Declared(Weights(self.checkpoint, 'vae'), blocks=[])
        self.model = request.memory.load(self.block, 'ram')

    def expected(self, x, rotary, batch):
        self.reference.to(x.device)
        try:
            with torch.no_grad(), torch.autocast('cuda', dtype=torch.float16,
                                               enabled=x.device.type == 'cuda'):
                return torch.cat([self.reference(x[start:start + batch], rotary).cpu()
                                  for start in range(0, x.shape[0], batch)], dim=0)
        finally:
            self.reference.to('cpu')

    def run(self, x, rotary):
        return _BlockRows(self.block, rotary).run(x)

    def close(self):
        self.model.close()
        self.checkpoint.close()


def selected(device):
    from cozy_runtime.internal.encoding.selection import measure_device

    facts = measure_device(torch, device.index or 0, device.type)
    ready = attention.pinned('kitchen-int8', facts, wait=True)[0]
    return attention._member(ready.candidate), ready


def rotary(device, tokens):
    positions = torch.linspace(-.95, .95, tokens * 3, dtype=torch.float32).view(1, tokens, 3)
    module = MiniMaxH3VideoRotaryPosEmbed(48).to(device)
    result = module(positions.to(device))
    assert all(tuple(t.shape) == (1, tokens, 1, 48) and t.dtype is torch.float32 for t in result)
    return result
