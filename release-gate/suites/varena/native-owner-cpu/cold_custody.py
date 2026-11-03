"""Actual native allocator callbacks refuse an unactivated device without CUDA/Torch."""
import argparse
import ctypes as C
import hashlib
import inspect
import json
import os
import sys
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--overlay', type=Path, required=True)
parser.add_argument('--out', type=Path, required=True)
args = parser.parse_args()
sys.path.insert(0, str(args.overlay.resolve()))
from tensorfs import _ext
from tensorfs.varena import _runtime as varena
from tensorfs.varena import torch as vt

assert varena.CooperationOwner.custody_capability() == 1
assert inspect.signature(vt.install).parameters['device'].default == 0
for function in (vt.install, vt.initialize):
    try:
        function(None)
    except TypeError:
        pass
    else:
        raise AssertionError('None still selected public CPU-only installation')
assert 'torch' not in sys.modules
assert vt._owner == 0


class Failure(C.Structure):
    _fields_ = [('kind', C.c_int32), ('cuda', C.c_int32), ('message', C.c_char * 512)]


class Bindings(C.Structure):
    _fields_ = [('abi', C.c_uint32), ('size', C.c_uint32), ('functions', C.c_void_p * 7)]


table = Bindings.from_address(_ext._varena_working_bindings())
assert table.abi == 1 and table.size == C.sizeof(Bindings)
alloc = C.CFUNCTYPE(C.c_uint64, C.c_size_t, C.c_int32, C.c_uint64, C.c_int32,
                   C.c_uint64, C.c_uint64, C.POINTER(Failure))(table.functions[0])
pool_begin = C.CFUNCTYPE(C.c_int32, C.c_int32, C.c_uint64, C.c_uint64,
                        C.c_uint64, C.POINTER(Failure))(table.functions[4])
refusals = []
for device in (0, 7):
    for count in (0, 1, 2 << 20):
        error = Failure()
        pointer = alloc(count, device, 0, 0, 0, 0, C.byref(error))
        message = error.message.decode()
        assert pointer == 0 and error.kind == 3 and 'activate' in message, (pointer, error.kind, message)
        refusals.append({'device': device, 'bytes': count, 'kind': error.kind, 'message': message})
    error = Failure()
    assert pool_begin(device, 1, 1, 0, C.byref(error)) == 0
    assert error.kind == 3 and 'activate' in error.message.decode()

maps = Path('/proc/self/maps').read_text()
assert 'libcuda.so' not in maps and 'libnvidia-ml.so' not in maps
assert 'torch' not in sys.modules
descriptors = []
for path in Path('/proc/self/fd').iterdir():
    try:
        value = os.readlink(path)
    except FileNotFoundError:
        continue
    if value.startswith('/dev/nvidia'):
        descriptors.append(value)
assert not descriptors
libraries = sorted({line.split(maxsplit=5)[-1] for line in maps.splitlines()
                    if len(line.split(maxsplit=5)) == 6 and ('libvarena' in line or 'libstd-' in line)})
result = {'passed': True, 'scope': 'exact native callback table and public None refusal, no simulated GPU',
          'extension': str(_ext.__file__), 'extension_sha256': hashlib.sha256(Path(_ext.__file__).read_bytes()).hexdigest(),
          'libraries': {path: hashlib.sha256(Path(path).read_bytes()).hexdigest() for path in libraries},
          'cuda_or_nvml_loaded': False, 'torch_imported': False, 'refusals': refusals,
          'pool_begin_unactivated_refused': True, 'public_install_default': 0,
          'public_none_refused_before_binding': True, 'custody_protocol': 1}
args.out.write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result, indent=2))
