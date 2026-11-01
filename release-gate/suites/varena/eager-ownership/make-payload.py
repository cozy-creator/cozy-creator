"""Exact wheel provenance; no installation or GPU work."""
from email.parser import BytesParser
import hashlib
import json
from pathlib import Path
import sys
from zipfile import ZipFile

runtime, tensorfs, gpu_uuid, destination = sys.argv[1:]
out = Path(destination).resolve()
assert not out.is_relative_to(Path(__file__).resolve().parent)
assert gpu_uuid.startswith('GPU-')

def version(archive):
    metadata, = [name for name in archive.namelist() if name.endswith('.dist-info/METADATA')]
    return BytesParser().parsebytes(archive.read(metadata))['Version']

def digest(archive, name):
    return hashlib.sha256(archive.read(name)).hexdigest()

with ZipFile(runtime) as r, ZipFile(tensorfs) as t:
    extension, = [name for name in t.namelist() if name.startswith('tensorfs/_ext.') and name.endswith('.so')]
    payload = {'runtime_version': version(r),
               'ownership_sha256': digest(r, 'cozy_runtime/internal/execution/ownership.py'),
               'loop_sha256': digest(r, 'cozy_runtime/internal/loop.py'),
               'tensorfs_version': version(t),
               'extension_sha256': digest(t, extension),
               'shim_sha256': digest(t, 'tensorfs/_varena_torch.so'),
               'gpu_uuid': gpu_uuid}
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(payload, indent=2) + '\n')
print(out)
