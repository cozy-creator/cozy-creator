"""Bind the actual two wheels and explicitly owned GPU; no installation or GPU work."""
import hashlib
import json
import sys
from email.parser import BytesParser
from pathlib import Path
from zipfile import ZipFile

runtime_path, tensorfs_path, uuid, output_path = sys.argv[1:]
output = Path(output_path).resolve()
assert not output.is_relative_to(Path(__file__).resolve().parent)
assert uuid.startswith(('GPU-', 'MIG-'))

def version(archive):
    path, = [name for name in archive.namelist() if name.endswith('.dist-info/METADATA')]
    return BytesParser().parsebytes(archive.read(path))['Version']

with ZipFile(runtime_path) as runtime, ZipFile(tensorfs_path) as tensorfs:
    extension, = [name for name in tensorfs.namelist() if name.startswith('tensorfs/_ext.') and name.endswith('.so')]
    result = {'runtime_version': version(runtime),
              'runtime_owner_sha256': hashlib.sha256(runtime.read('cozy_runtime/internal/cooperation.py')).hexdigest(),
              'tensorfs_version': version(tensorfs),
              'extension_sha256': hashlib.sha256(tensorfs.read(extension)).hexdigest(),
              'shim_sha256': hashlib.sha256(tensorfs.read('tensorfs/_varena_torch.so')).hexdigest(),
              'gpu_uuid': uuid}
output.write_text(json.dumps(result, indent=2) + '\n')
print(output)
