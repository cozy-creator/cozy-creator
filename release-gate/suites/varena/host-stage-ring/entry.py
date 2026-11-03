"""Ready bundle entry: receives exactly the original SDXL client's arguments."""
import hashlib
import json
import os
import pathlib
import sys


def main():
    here = pathlib.Path(__file__).resolve().parent
    binding = json.loads((here / 'binding.json').read_text())
    for name, expected in binding['diagnostic_files'].items():
        if hashlib.sha256((here / name).read_bytes()).hexdigest() != expected:
            raise RuntimeError(f'diagnostic file identity mismatch: {name}')
    sys.path.insert(0, str(here))  # Required for the operator's isolated (-I) child.
    from host_stage_observe import run_once
    from native_markers import NativeTrace
    import torch
    import tensorfs
    if torch.__version__ != '2.15.0+cu132' or torch.version.git_version != binding['torch_git']:
        raise RuntimeError('GPU diagnostic Torch cohort mismatch')
    if torch.version.debug or not torch._C._GLIBCXX_USE_CXX11_ABI:
        raise RuntimeError('GPU diagnostic Torch build mode mismatch')
    if torch.cuda.is_initialized():
        raise RuntimeError('host-stage observer must attach before CUDA initialization')
    root = pathlib.Path(tensorfs.__file__).resolve().parent
    for name, expected in binding['native_files'].items():
        if hashlib.sha256((root / name).read_bytes()).hexdigest() != expected:
            raise RuntimeError(f'diagnostic native image mismatch: {name}')
    native = NativeTrace(here / 'allocation_trace.so', here / 'diagnostic_markers.h', binding['helper_sha256'])
    latched = native.workspace_cache()
    if latched:
        raise RuntimeError('async graph diagnostic requires actual latched workspace cache disabled')
    if os.environ.get('CUDA_LAUNCH_BLOCKING') not in (None, '', '0'):
        raise RuntimeError('async diagnostic must not enable CUDA_LAUNCH_BLOCKING')
    arguments = sys.argv[1:]
    output = pathlib.Path(arguments[arguments.index('--output') + 1])
    observed = dict(binding)
    observed['TORCH_CUBLAS_WORKSPACE_CACHE_environment'] = os.environ.get('TORCH_CUBLAS_WORKSPACE_CACHE')
    observed['environment_is_latched_Torch_value'] = False
    observed['actual_latched_workspace_cache'] = latched
    observed['CUDA_LAUNCH_BLOCKING'] = os.environ.get('CUDA_LAUNCH_BLOCKING')
    run_once(binding['cohort'], here / 'original_sdxl.py',
             output / 'host-stage-ring.json', arguments, observed, native)


if __name__ == '__main__':
    main()
