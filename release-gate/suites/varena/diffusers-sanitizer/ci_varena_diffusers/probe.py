"""Fresh instrumented child using only public Diffusers/Torch/TensorFS APIs."""
from __future__ import annotations

import gc
import hashlib
from importlib.metadata import version
import json
from pathlib import Path
import runpy
import sys


def digest(path):
    with Path(path).open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def emit(value):
    print(json.dumps(value, sort_keys=True), flush=True)


def main(config):
    import torch
    import tensorfs
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    assert not torch.cuda.is_initialized(), 'fresh child initialized CUDA before allocator installation'
    actual = {'tensorfs_version': version('tensorfs'), 'extension_sha256': digest(_ext.__file__),
              'shim_sha256': digest(Path(tensorfs.__file__).resolve().parent / '_varena_torch.so'),
              'torch': str(torch.__version__), 'torch_git': torch.version.git_version,
              'torch_distribution': version('torch'), 'cuda': torch.version.cuda,
              'python': sys.version, 'executable': sys.executable}
    for key in ('tensorfs_version', 'extension_sha256', 'shim_sha256'):
        assert actual[key] == config[key], (key, actual[key], config[key])
    assert actual['torch'] == '2.14.0+cu130' and actual['torch_distribution'] == '2.14.0'
    assert actual['torch_git'] == '08187d9e0fba026dc8217405802ab5381dc88d90' and actual['cuda'] == '13.0'
    dependencies = {'diffusers': '0.41.0', 'transformers': '5.17.0', 'accelerate': '1.15.0',
                    'huggingface-hub': '1.33.0', 'safetensors': '0.8.0', 'numpy': '2.5.3'}
    actual['dependencies'] = {name: version(name) for name in dependencies}
    assert actual['dependencies'] == dependencies, actual['dependencies']
    torch.backends.cudnn.benchmark = False
    if config['mode'] == 'varena':
        vt.install()  # Before CUDA initialization and before importing the public pipeline.
    else:
        assert config['mode'] == 'native' and torch.cuda.get_allocator_backend() == 'native'
    assert not torch.cuda.is_initialized()
    from diffusers import StableDiffusionXLPipeline
    import numpy as np

    manifest = runpy.run_path(str(Path(__file__).with_name('manifest.py')))['MANIFEST']
    snapshot = Path(config['snapshot']).resolve()
    assert snapshot.name == manifest['revision'], snapshot
    for item in manifest['files']:
        assert (snapshot / item['path']).stat().st_size == item['bytes']
    torch.set_num_threads(1)
    torch.cuda.set_device(0)
    actual.update({'gpu': torch.cuda.get_device_name(0), 'gpu_uuid': str(torch.cuda.get_device_properties(0).uuid),
                   'cudnn': torch.backends.cudnn.version(), 'mode': config['mode']})
    assert actual['gpu_uuid'] == config['gpu_uuid'], actual['gpu_uuid']
    emit({'provenance': actual})
    pipe = StableDiffusionXLPipeline.from_pretrained(
        str(snapshot), torch_dtype=torch.float16, variant='fp16', use_safetensors=True,
        local_files_only=True, add_watermarker=False,
    ).to('cuda')
    pipe.set_progress_bar_config(disable=True)
    parameters = {name: sum(parameter.numel() for parameter in getattr(pipe, name).parameters())
                  for name in ('text_encoder', 'text_encoder_2', 'unet', 'vae')}
    torch.cuda.reset_peak_memory_stats(0)
    generator = torch.Generator(device='cpu').manual_seed(config['seed'])

    def progress(_pipe, step, _time, callback_kwargs):
        emit({'denoise_step': step + 1})
        return callback_kwargs

    with torch.inference_mode():
        result = pipe(prompt='a red fox in snow, a photograph', height=1024, width=1024,
                      num_inference_steps=2, guidance_scale=5.0, generator=generator,
                      output_type='np', callback_on_step_end=progress)
    image = result.images
    assert image.shape == (1, 1024, 1024, 3) and np.isfinite(image).all(), image.shape
    image_receipt = {'shape': list(image.shape), 'dtype': str(image.dtype),
                     'sha256': hashlib.sha256(image.tobytes()).hexdigest(),
                     'min': float(image.min()), 'max': float(image.max())}
    peak = {'allocated_bytes': torch.cuda.max_memory_allocated(0), 'reserved_bytes': torch.cuda.max_memory_reserved(0)}
    del image, result, pipe, generator
    gc.collect()
    torch.cuda.synchronize()
    # Torch uses this same allocator-owned workspace cleanup in its CUDA leak checks.
    torch._C._cuda_clearCublasWorkspaces()
    # First trim seals deferred frees; synchronization then lets the second trim retire them.
    torch.cuda.empty_cache()
    torch.cuda.synchronize()
    torch.cuda.empty_cache()
    host_before = torch.cuda.host_memory_stats()
    torch.accelerator.empty_host_cache()
    host_after = torch.cuda.host_memory_stats()
    assert host_after['active_bytes.current'] == host_after['allocated_bytes.current'] == 0, host_after
    if config['mode'] == 'varena':
        final = vt.stats(0)
        assert final['allocations'] > 0
        for name in ('allocated_bytes', 'mapped_bytes', 'pending_bytes', 'pools', 'event_handles'):
            assert final[name] == 0, final
    else:
        final = {'allocated_bytes': torch.cuda.memory_allocated(0), 'reserved_bytes': torch.cuda.memory_reserved(0)}
        assert not any(final.values()), final
    emit({'result': {'passed': True, 'provenance': actual, 'model_revision': manifest['revision'],
                     'parameters': parameters, 'image': image_receipt, 'steps': 2,
                     'peak': peak, 'final': final, 'host_cache': {'before': host_before, 'after': host_after},
                     'scope': 'managed-memory cleanup passed; final process-exit sanitizer verdict remains separate'}})


if __name__ == '__main__':
    try:
        main(json.loads(sys.argv[1]))
    except BaseException as error:
        emit({'result': {'passed': False, 'error': f'{type(error).__name__}: {error}'}})
        raise
