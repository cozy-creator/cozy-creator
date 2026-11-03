"""An uncooperative stand-in game: default Torch allocator, no Varena participant."""
import json
import os
import sys
import time
import traceback


def emit(value):
    print(json.dumps(value, sort_keys=True), flush=True)


def main():
    import torch
    config = json.loads(sys.argv[1])
    assert torch.__version__ == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    assert not torch.cuda.is_initialized()
    emit({'ready': True, 'pid': os.getpid(), 'cuda_initialized': False, 'participant': False})
    assert input() == 'ask'
    start = time.monotonic_ns()
    value = torch.full((config['bytes'],), 71, dtype=torch.uint8, device='cuda')
    assert int(value[0]) == int(value[-1]) == 71
    torch.cuda.synchronize()
    free, total = torch.cuda.mem_get_info()
    emit({'passed': True, 'pid': os.getpid(), 'allocation_start_ns': start,
          'allocation_complete_ns': time.monotonic_ns(), 'bytes': value.numel(),
          'driver_free': free, 'driver_total': total,
          'torch_reserved': torch.cuda.memory_reserved(), 'participant': False})
    assert input() == 'quit'
    assert int(value[0]) == int(value[-1]) == 71
    del value
    torch.cuda.synchronize(); torch.cuda.empty_cache()
    emit({'passed': True, 'released': True, 'reserved': torch.cuda.memory_reserved()})


try:
    main()
except BaseException as error:
    emit({'passed': False, 'error': f'{type(error).__name__}: {error}', 'traceback': traceback.format_exc()})
    raise
