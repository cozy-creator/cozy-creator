"""Real tiny Diffusers encode/decode entrypoints; never labels them root-forward coverage."""
from __future__ import annotations

import argparse
import json
import torch
from diffusers import AutoencoderKL
from tensorfs.varena import torch as vt
from tensorfs.varena._offload_stream import component_scope

parser = argparse.ArgumentParser()
parser.add_argument('--device', type=int, default=0)
parser.add_argument('--gpu-uuid', required=True)
args = parser.parse_args()
vt.install(args.device)
torch.cuda.set_device(args.device)
normalize = lambda x: str(x).removeprefix('GPU-').replace('-', '').lower()
assert normalize(torch.cuda.get_device_properties(args.device).uuid) == normalize(args.gpu_uuid)
torch.manual_seed(13)


class ObservedVAE(AutoencoderKL):
    forward_calls = 0
    def forward(self, *args, **kwargs):
        self.forward_calls += 1
        return super().forward(*args, **kwargs)


vae = ObservedVAE(in_channels=3, out_channels=3, down_block_types=('DownEncoderBlock2D',),
                  up_block_types=('UpDecoderBlock2D',), block_out_channels=(8,), layers_per_block=1,
                  latent_channels=4, norm_num_groups=4, sample_size=8, force_upcast=True).eval()
image = torch.linspace(-1, 1, 3 * 8 * 8, device=args.device).reshape(1, 3, 8, 8)
with torch.inference_mode():
    vae.to(device=args.device)
    expected_latent = vae.encode(image).latent_dist.mode().clone()
    expected = vae.decode(expected_latent).sample.clone()
    vae.to('cpu')
    torch.cuda.current_stream(args.device).synchronize()
    handle = vt.offload(vae, device=args.device)
    try:
        with component_scope(handle._owner, (image,)) as encode:
            encoded = vae.encode(image)
            distribution = encoded.latent_dist
            assert encode.output(encoded, values=(distribution.parameters, distribution.mean,
                distribution.logvar, distribution.std, distribution.var)) is encoded
        encode.completion.wait()
        latent = encoded.latent_dist.mode()
        with component_scope(handle._owner, (latent,)) as decode:
            decoded = vae.decode(latent)
            assert decode.output(decoded, values=(decoded.sample,)) is decoded
        decode.completion.wait()
        result = decoded.sample
        torch.testing.assert_close(latent, expected_latent)
        torch.testing.assert_close(result, expected)
        assert vae.forward_calls == 0 and vae.config.force_upcast
        assert encode.completion.can_train and decode.completion.can_train
    finally:
        torch.cuda.current_stream(args.device).synchronize()
        handle.close()
print(json.dumps({'passed': True, 'entrypoints': ['encode', 'decode'], 'root_forward_calls': vae.forward_calls,
                  'shape': list(result.shape), 'real_diffusers_vae': True, 'model_downloads': False,
                  'scope': 'explicit method scopes; no full SDXL, automatic wrapper or speed claim'}))
