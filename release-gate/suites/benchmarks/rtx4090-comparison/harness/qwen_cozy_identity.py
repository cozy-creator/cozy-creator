# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.33,<0.34", "msgspec>=0.19", "diffusers==0.41.0", "torch==2.14.1", "torchvision==0.29.1", "transformers==5.17.0", "pillow==12.3.0", "tokenizers==0.23.2", "numpy==2.5.3", "safetensors==0.8.0"]
# ///
"""Untimed real Qwen2.1 tensor/schedule identity through the ordinary Weights grant."""
import json
import time
from pathlib import Path

import msgspec

from cozy_runtime.author import App, Context, Weights

"""Canonical real-tensor receipt helpers for one Qwen2.1 comparison; no quantization."""
import hashlib


def tensor_identity(tensor):
    import torch
    tensor = tensor.detach().cpu().contiguous()
    names = {torch.float32:'F32', torch.bfloat16:'BF16', torch.float16:'F16',
             torch.int8:'I8', torch.uint8:'U8', torch.int64:'I64', torch.int32:'I32'}
    dtype = names[tensor.dtype]
    raw, values = hashlib.sha256(), hashlib.sha256()
    flat = tensor.reshape(-1)
    floating = tensor.is_floating_point()
    for start in range(0, flat.numel(), 1 << 20):
        part = flat[start:start + (1 << 20)]
        raw.update(part.view(torch.uint8).numpy().tobytes())
        if floating:
            as_float = part.float()
            assert torch.isfinite(as_float).all(), 'nonfinite stored weight'
            values.update(as_float.numpy().tobytes())
    row = {'dtype':dtype, 'shape':list(tensor.shape), 'sha256':raw.hexdigest()}
    if floating: row['float32_sha256'] = values.hexdigest()
    return row


def plain_identity(tensor):
    return {'encoding':None, **tensor_identity(tensor)}


def convrot_identity(weight, scale, params, logical_dtype='BF16'):
    import torch
    assert weight.dtype == torch.int8 and weight.ndim == 2
    assert set(params) == {'rotation', 'rotation_size'}
    assert params['rotation'] == 'kron_regular_h4' and params['rotation_size'] == 256
    assert weight.shape[1] % 256 == 0 and scale.numel() == weight.shape[0]
    assert scale.dtype == torch.float32, ('scale dtype differs from frozen input',scale.dtype)
    return {'dtype':logical_dtype, 'shape':list(weight.shape),
            'encoding':{'scheme':'int8.convrot','params':params},
            'parts':{'weight':tensor_identity(weight), 'scale':tensor_identity(scale.reshape(-1))}}


EXPECTED = "sha256:78705de45f7421fde51a0196913b11f1f7e3c95d468881d762d18af50da6fec3"

app = App()


class QwenIdentityRequest(msgspec.Struct):
    folder: str = ""
    export: bool = False
    expected_runtime_source: str = "774624133be6d74262f845b9358874872de100d9"
    dependencies_only: bool = False


class QwenIdentityResult(msgspec.Struct):
    identity: str


@app.entrypoint(accelerator=False)
def main(payload: QwenIdentityRequest, weights: Weights, ctx: Context) -> QwenIdentityResult:
    folder = payload.folder or "/var/tmp/bench4090-20261026"
    export, expected_runtime_source = payload.export, payload.expected_runtime_source
    import numpy as np
    import torch
    import tensorfs
    from diffusers import FlowMatchEulerDiscreteScheduler
    from diffusers.pipelines.qwenimage21.pipeline_qwenimage21 import calculate_shift
    from cozy_runtime._build_provenance import COMMIT
    began = time.perf_counter()
    assert len(expected_runtime_source) == 40 and all(c in "0123456789abcdef" for c in expected_runtime_source), expected_runtime_source
    assert COMMIT == expected_runtime_source, (COMMIT, expected_runtime_source)
    if payload.dependencies_only:
        from importlib.metadata import version
        schedule = FlowMatchEulerDiscreteScheduler()
        mu = calculate_shift(4096, 256, 4096, 0.5, 1.15)
        schedule.set_timesteps(30, device="cpu", sigmas=np.linspace(1.0, 1 / 30, 30), mu=mu)
        assert len(schedule.sigmas) == 31 and not torch.cuda.is_initialized()
        return QwenIdentityResult(identity=json.dumps({
            "dependencies_only": True, "runtime_commit": COMMIT,
            "torch": str(torch.__version__), "torch_git": torch.version.git_version,
            "versions": {name: version(name) for name in
                         ("diffusers", "transformers", "torchvision", "pillow", "tokenizers", "numpy", "safetensors")},
            "scheduler_sigmas": schedule.sigmas.tolist(), "cuda_initialized": False,
            "scope": "Actual identity-body imports and CPU schedule; no TensorFS source read, GPU inference or tensor-identity claim.",
        }))
    source = ctx.tensorfs_source(weights)
    assert source.id == EXPECTED, source.id
    dtypes = {'F32':torch.float32,'BF16':torch.bfloat16,'F16':torch.float16,
              'I8':torch.int8,'U8':torch.uint8,'I64':torch.int64,'I32':torch.int32}
    def as_torch(array):
        return torch.frombuffer(bytearray(array.tobytes()), dtype=dtypes[array.dtype]).reshape(array.shape)
    rows = {}
    total = 0
    for component in ('transformer','text_encoder','vae'):
        rows[component] = {}
        for key, metadata in source.components[component].tensors.items():
            ctx.raise_if_cancelled()
            value = source.components[component].tensors.read(key)
            if isinstance(value, tensorfs.Encoded):
                assert value.scheme == 'int8.convrot' and set(value.parts) == {'weight','scale'}, (component,key,value.scheme)
                row = convrot_identity(as_torch(value.parts['weight']), as_torch(value.parts['scale']), value.params, value.dtype)
                total += sum(part.nbytes for part in value.parts.values()) if all(hasattr(part,'nbytes') for part in value.parts.values()) else sum(len(part.tobytes()) for part in value.parts.values())
            else:
                row = plain_identity(as_torch(value))
                total += len(value.tobytes())
            rows[component][key] = row
            del value
    config = dict(source.components['scheduler'].config)
    model_index = source.components.get('model_index')
    if model_index is not None:
        assert not model_index.config.get('sample_sigmas'), 'distilled checkpoint schedule is not the base workload'
    schedule = FlowMatchEulerDiscreteScheduler.from_config(config)
    sequence = (1024 // 16) ** 2
    mu = calculate_shift(sequence, config.get('base_image_seq_len',256),
                         config.get('max_image_seq_len',4096), config.get('base_shift',0.5), config.get('max_shift',1.15))
    schedule.set_timesteps(30, device='cpu', sigmas=np.linspace(1.0,1/30,30), mu=mu)
    assert len(schedule.sigmas) == 31
    assert source.verify().ok
    root = Path(folder); root.mkdir(parents=True,exist_ok=True)
    exported = None
    if export:
        # Existing bounded native exporter preserves every encoded part. No new quantizer.
        exported = tensorfs.materialize(source,local_dir=root/'qwen-cozy-export',format='safetensors',decode=False)
    assert not torch.cuda.is_initialized(), 'CPU identity unexpectedly initialized CUDA'
    result = {'tensorfs_checkpoint':source.id,'components':rows,'scheduler_config':config,
              'mu':mu,'latent_sequence_length':sequence,'sigmas':schedule.sigmas.tolist(),
              'timesteps':schedule.timesteps.tolist(),'steps':30,'width':1024,'height':1024,
              'torch':str(torch.__version__),'torch_git':torch.version.git_version,
              'runtime_commit':COMMIT,'tensorfs':tensorfs.__version__,
              'stored_tensor_bytes_observed':total,'seconds':time.perf_counter()-began,'exported':exported,
              'scope':'Real complete stored tensor reads and exact library scheduler on CPU; no inference. Reads warm OS pages.'}
    path=root/'qwen-cozy-identity.json';path.write_text(json.dumps(result,sort_keys=True))
    return QwenIdentityResult(identity=json.dumps(result))
