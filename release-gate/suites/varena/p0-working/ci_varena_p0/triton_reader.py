"""Actual public Triton pointer reader and do_bench allocator consumer, in a fresh process."""
from __future__ import annotations

import gc
import hashlib
from importlib.metadata import version
import json
from pathlib import Path
import sys

import torch
import triton
import triton.language as tl
import triton.testing
import tensorfs
from tensorfs import _ext
from tensorfs.varena import torch as vt


@triton.jit
def affine(source, output, count: tl.constexpr, BLOCK: tl.constexpr):
    offset = tl.program_id(0) * BLOCK + tl.arange(0, BLOCK)
    value = tl.load(source + offset, offset < count, other=0)
    tl.store(output + offset, value * 2 + 3, offset < count)


def digest(path):
    with Path(path).open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def emit(value):
    print(json.dumps(value, sort_keys=True), flush=True)


def main(config):
    assert not torch.cuda.is_initialized(), 'fresh child must install before CUDA'
    actual = {'tensorfs_version': version('tensorfs'), 'extension_sha256': digest(_ext.__file__),
              'shim_sha256': digest(Path(tensorfs.__file__).resolve().parent / '_varena_torch.so'),
              'torch': str(torch.__version__), 'torch_git': torch.version.git_version,
              'triton': version('triton'), 'triton_testing_sha256': digest(triton.testing.__file__)}
    for key in ('tensorfs_version', 'extension_sha256', 'shim_sha256'):
        assert actual[key] == config[key], (key, actual[key], config[key])
    assert actual['torch'] == config['torch_version'] and actual['torch_git'] == config['torch_git']
    assert actual['triton'] == '3.8.0'
    mode = config['case']
    if mode == 'varena':
        vt.install()
    else:
        assert mode == 'native' and torch.cuda.get_allocator_backend() == 'native'
    assert not torch.cuda.is_initialized()
    torch.set_num_threads(1)
    torch.cuda.set_device(0)
    actual.update({'gpu': torch.cuda.get_device_name(0), 'gpu_uuid': str(torch.cuda.get_device_properties(0).uuid),
                   'mode': mode, 'executable': sys.executable, 'cuda': torch.version.cuda})
    emit({'provenance': actual})
    count = 65536
    source = torch.arange(count, dtype=torch.float32, device='cuda')
    output = torch.empty_like(source)
    expected = torch.arange(count, dtype=torch.float32) * 2 + 3

    def launch():
        affine[(triton.cdiv(count, 256),)](source, output, count, BLOCK=256)

    launch()
    torch.cuda.synchronize()
    assert torch.equal(output.cpu(), expected)
    torch.cuda.reset_peak_memory_stats(0)
    # Public do_bench allocates its own256MiB Torch CUDA cache buffer, then clears it before launches.
    milliseconds = triton.testing.do_bench(launch, warmup=1, rep=3)
    assert torch.equal(output.cpu(), expected)
    peak = torch.cuda.max_memory_allocated(0)
    assert peak >= 256 * 1024 * 1024, 'do_bench did not exercise its actual CUDA allocation'
    side = torch.cuda.Stream()
    side.wait_stream(torch.cuda.current_stream())
    with torch.cuda.stream(side):
        launch()
        source.record_stream(side)
        output.record_stream(side)
    torch.cuda.synchronize()
    host = output.cpu()
    assert torch.equal(host, expected)
    output_sha = hashlib.sha256(host.numpy().tobytes()).hexdigest()
    del source, output, host, expected, side
    gc.collect()
    torch.cuda.empty_cache()
    torch.cuda.synchronize()
    torch.cuda.empty_cache()
    torch.accelerator.empty_host_cache()
    host_stats = torch.cuda.host_memory_stats()
    assert host_stats['active_bytes.current'] == host_stats['allocated_bytes.current'] == 0
    if mode == 'varena':
        final = vt.stats(0)
        for key in ('allocated_bytes', 'mapped_bytes', 'pending_bytes', 'pools', 'event_handles'):
            assert final[key] == 0, final
        assert final['allocations'] > 0
    else:
        final = {'allocated_bytes': torch.cuda.memory_allocated(0), 'reserved_bytes': torch.cuda.memory_reserved(0)}
        assert not any(final.values()), final
    emit({'result': {'passed': True, 'provenance': actual, 'output_sha256': output_sha,
                     'elements': count, 'default_and_side_stream_output_correct': True,
                     'do_bench_ms': milliseconds, 'peak_allocated_bytes': peak, 'final': final,
                     'scope': 'public Triton real CUDA reader and256MiB allocation adoption; timing not a performance gate'}})


if __name__ == '__main__':
    try:
        main(json.loads(sys.argv[1]))
    except BaseException as error:
        emit({'result': {'passed': False, 'error': f'{type(error).__name__}: {error}'}})
        raise
