"""Portable native API and CPU-parent fork check; never constructs a CUDA owner."""
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import sys

sys.path.insert(0, sys.argv[1])
from tensorfs import _ext
from tensorfs.varena import _runtime as varena

assert varena.CooperationOwner.capability() == 1
methods = ('wake', 'facts', 'configure', 'snapshot', 'cuda_calls', 'close')
assert all(callable(getattr(_ext._VarenaOwner, name)) for name in methods)
varena.CooperationOwner.initialize()

def cold():
    maps = Path('/proc/self/maps').read_text()
    assert 'libcuda.so' not in maps and 'libnvidia-ml.so' not in maps
    assert 'torch' not in sys.modules
    tasks = {p.name: (p / 'comm').read_text().strip() for p in Path('/proc/self/task').iterdir()}
    assert not any(name.startswith('varena-owner') for name in tasks.values())
    return {'pid': os.getpid(), 'tasks': tasks, 'torch_imported': False, 'driver_loaded': False}

parent = cold()
read, write = os.pipe()
child = os.fork()
if child == 0:
    os.close(read)
    try:
        varena.CooperationOwner.initialize()
        os.write(write, json.dumps(cold()).encode())
        os._exit(0)
    except BaseException:
        os._exit(1)
os.close(write)
if not select.select([read], [], [], 5)[0]:
    os.kill(child, signal.SIGKILL)
    os.waitpid(child, 0)
    raise AssertionError('CPU-parent fork metadata did not return; harness failure')
data = os.read(read, 16384)
os.close(read)
_, status = os.waitpid(child, 0)
assert os.waitstatus_to_exitcode(status) == 0
report = {'passed': True, 'extension': _ext.__file__,
          'extension_sha256': hashlib.sha256(Path(_ext.__file__).read_bytes()).hexdigest(),
          'native_version': _ext.__version__, 'owner_protocol': _ext._VarenaOwner.OWNER_PROTOCOL,
          'methods': methods, 'parent': parent, 'child': json.loads(data),
          'scope': 'portable API and metadata-only CPU-parent fork; no GPU owner, CUDA or donor execution'}
Path(sys.argv[2]).write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, sort_keys=True))
