"""Exact installed cohort inputs; write outside this immutable source tree."""
import hashlib
import json
import sys
import zipfile
from email.parser import BytesParser
from pathlib import Path

runtime, tensorfs, uuid, output = sys.argv[1:]
root = Path(__file__).resolve().parent
out = Path(output).resolve()
if out == root or root in out.parents:
    raise SystemExit('write the payload outside frozen source')


def package(path, name):
    archive = zipfile.ZipFile(path)
    metadata = next(n for n in archive.namelist() if n.endswith('.dist-info/METADATA'))
    version = BytesParser().parsebytes(archive.read(metadata))['Version']
    return archive, version


def digest(archive, name):
    return hashlib.sha256(archive.read(name)).hexdigest()


r, rv = package(runtime, 'cozy-runtime')
t, tv = package(tensorfs, 'tensorfs')
modules = ('author/kernels.py', 'author/_split.py', 'internal/encoding/leaves.py',
           'internal/prebuilt.py', 'internal/execution/__init__.py',
           'internal/execution/native_linear_split.py', 'lora.py', '_kernels/_gemm.abi3.so')
result = {'runtime_version': rv,
          'runtime_modules': {n: digest(r, 'cozy_runtime/' + n) for n in modules},
          'tensorfs_version': tv, 'extension_sha256': digest(t, 'tensorfs/_ext.abi3.so'),
          'shim_sha256': digest(t, 'tensorfs/_varena_torch.so'), 'gpu_uuid': uuid,
          'seed': 13818}
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(result, indent=2) + '\n')
print(out)
