"""Actual adopted CPU models with the committed component helper/shared source.

Host API/capture-binding evidence only. No CUDA stream/event/resource substitute.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from types import SimpleNamespace

import torch
from diffusers import AutoencoderKL
from tensorfs.varena import _offload as adapter
from tensorfs.varena import torch as vt
from tensorfs.varena._offload_graph import _CaptureBindings
from tensorfs.varena._offload_stream import component_scope, reuse

torch.set_num_threads(1)
torch.set_num_interop_threads(1)
torch.manual_seed(37)
model = torch.nn.Sequential(
    torch.nn.Linear(4, 4), torch.nn.ReLU(), torch.nn.Linear(4, 2)
).eval()
inputs = torch.arange(12.0).reshape(3, 4)
with torch.inference_mode():
    expected = model(inputs)
handle = vt.offload(model, device="cpu")
device = handle._owner
assert not reuse(device, None)
with torch.inference_mode(), component_scope(device, (inputs,)) as outer:
    assert reuse(device, None)
    checkpoint = outer.checkpoint()
    checkpoint.wait()
    assert checkpoint.checked and not checkpoint.can_train
    with component_scope(device, (inputs,)) as nested:
        assert nested.reused
        result = model(inputs)
        opaque = SimpleNamespace(sample=result, view=result.view(-1))
        assert nested.output(opaque, values=(opaque.sample, opaque.view)) is opaque
        prefix = nested.checkpoint()
        prefix.wait()
        assert prefix.checked and not prefix.can_train
    assert reuse(device, None), "nested exit closed the outer scope"
    returned = {"sample": result, "alias": opaque.view}
    assert outer.output(returned) is returned
assert outer.completion.query() and not outer.completion.can_train
assert outer.completion.handoff(None, returned) is returned
torch.testing.assert_close(returned["sample"], expected, rtol=0, atol=0)
assert torch._C._is_alias_of(returned["sample"], returned["alias"])
assert outer._inputs == outer._outputs == nested._inputs == nested._outputs == ()
assert not reuse(device, None)

try:
    with component_scope(device, (inputs,)) as failed:
        proof = failed.checkpoint()
        proof.wait()
        raise ValueError("actual authored exception")
except ValueError as error:
    assert str(error) == "actual authored exception"
else:
    raise AssertionError("component helper swallowed an authored exception")
assert not reuse(device, None)
for witness in (proof, failed.completion):
    try:
        witness.query()
    except adapter.OffloadError:
        pass
    else:
        raise AssertionError("failed scope witness claimed successful completion")
try:
    outer.checkpoint()
except adapter.OffloadError:
    pass
else:
    raise AssertionError("closed scope acquired a new checkpoint")

versions = tuple(
    dict.fromkeys(parameter._version_owner for parameter in model.parameters())
)
prepared = {
    version: version.tensor(
        (version.source.nbytes // torch.float32.itemsize,), (1,), 0, None
    )
    for version in versions
}
binding = _CaptureBindings(handle, model, prepared, None, (inputs, {}))
materializations = []


def observe(frame, event, _):
    if event == "call" and frame.f_code is adapter._Version.tensor.__code__:
        materializations.append(True)


with torch.inference_mode():
    sys.setprofile(observe)
    try:
        with (
            binding.scope(),
            component_scope(device, (inputs,), capture_bindings=binding) as capture,
        ):
            assert capture.capture and not reuse(device, None)
            actual = capture.output(model(inputs))
            assert capture.checkpoint() is None
    finally:
        sys.setprofile(None)
torch.testing.assert_close(actual, expected, rtol=0, atol=0)
assert capture.completion is None and not materializations
binding.close()
prepared.clear()
with torch.inference_mode():
    direct = model[0].weight.clone()
assert direct.shape == (4, 4) and not reuse(device, None)
handle.close()


class ObservedVAE(AutoencoderKL):
    forward_calls = 0

    def forward(self, *args, **kwargs):
        self.forward_calls += 1
        return super().forward(*args, **kwargs)


vae = ObservedVAE(
    in_channels=3,
    out_channels=3,
    down_block_types=("DownEncoderBlock2D",),
    up_block_types=("UpDecoderBlock2D",),
    block_out_channels=(8,),
    layers_per_block=1,
    latent_channels=4,
    norm_num_groups=4,
    sample_size=8,
).eval()
image = torch.linspace(-1, 1, 192).reshape(1, 3, 8, 8)
with torch.inference_mode():
    expected_latent = vae.encode(image).latent_dist.mode()
    expected_image = vae.decode(expected_latent).sample
vae_handle = vt.offload(vae, device="cpu")
with torch.inference_mode():
    with component_scope(vae_handle._owner, (image,)) as encode:
        encoded = vae.encode(image)
        distribution = encoded.latent_dist
        assert (
            encode.output(
                encoded,
                values=(
                    distribution.parameters,
                    distribution.mean,
                    distribution.logvar,
                    distribution.std,
                    distribution.var,
                ),
            )
            is encoded
        )
    latent = distribution.mode()
    with component_scope(vae_handle._owner, (latent,)) as decode:
        decoded = vae.decode(latent)
        assert decode.output(decoded, values=(decoded.sample,)) is decoded
torch.testing.assert_close(latent, expected_latent, rtol=0, atol=0)
torch.testing.assert_close(decoded.sample, expected_image, rtol=0, atol=0)
assert vae.forward_calls == 0
vae_handle.close()
assert not torch.cuda.is_initialized()
assert "libcuda.so" not in Path("/proc/self/maps").read_text()
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "cuda": False,
            "actual_host_model_nested_scope_and_output_identity": True,
            "open_checkpoint_host_only_failed_proofs_refused": True,
            "capture_bindings_bypass_eager_witness_and_materialization": True,
            "direct_weight_read_preserved": True,
            "tiny_vae_encode_decode_exact_explicit_scopes": True,
            "vae_root_forward_calls": vae.forward_calls,
            "gpu_reuse_records_events_or_automatic_root_wrappers": "not exercised",
        }
    )
)
