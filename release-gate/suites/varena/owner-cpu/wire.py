"""Real native command decoding: old omission and new categorical Start record, no CUDA."""
import argparse
import hashlib
import json
import sys
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('overlay', type=Path)
parser.add_argument('output', type=Path)
args = parser.parse_args()
sys.path.insert(0, str(args.overlay.resolve()))
from tensorfs import _ext, executor
from tensorfs.varena import _runtime as varena

payload = {'cmd': 'start', 'devices': 'GPU-13800000-0000-0000-0000-000000000003',
           'application': 'fixture', 'package_interface': '/not-opened',
           'sequence_parallel_degree': 1,
           'cooperation': {'uuid': 'GPU-13800000-0000-0000-0000-000000000003',
                           'directory': '/dev/shm/not-opened', 'shared': True, 'priority': -4}}
start = executor.decode_command(json.dumps(payload).encode())
assert isinstance(start, executor.Start) and isinstance(start.cooperation, executor.Cooperation)
assert json.loads(start.json())['cooperation'] == payload['cooperation']
old = executor.decode_command(json.dumps({k: v for k, v in payload.items() if k != 'cooperation'}).encode())
assert old.cooperation is None
assert issubclass(varena.CooperationStaleSampleError, varena.CooperationDeferredError)
assert varena.Cooperation.capability() == 1
maps = Path('/proc/self/maps').read_text()
assert 'libcuda.so' not in maps and 'libnvidia-ml.so' not in maps
result = {'passed': True, 'typed_cooperation_roundtrip': True, 'old_start_defaults_none': True,
          'typed_stale_is_deferred': True, 'cuda_library_loaded': False,
          'extension_sha256': hashlib.sha256(Path(_ext.__file__).read_bytes()).hexdigest()}
args.output.write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result))
