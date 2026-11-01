"""Create exact candidate provenance from the wheels; no installation or GPU work."""
import hashlib
import json
import sys
from email.parser import BytesParser
from pathlib import Path
from zipfile import ZipFile


def wheel_version(archive):
    metadata = [name for name in archive.namelist() if name.endswith('.dist-info/METADATA')]
    assert len(metadata) == 1, metadata
    return BytesParser().parsebytes(archive.read(metadata[0]))['Version']


runtime_path, tensorfs_path, gpu_uuid, output_path = sys.argv[1:]
assert gpu_uuid.startswith('GPU-'), 'supply the explicitly owned rental GPU UUID'
output = Path(output_path).resolve()
assert not output.is_relative_to(Path(__file__).resolve().parent), 'keep payload outside frozen source'
with ZipFile(runtime_path) as runtime, ZipFile(tensorfs_path) as tensorfs:
    extensions = [name for name in tensorfs.namelist()
                  if name.startswith('tensorfs/_ext.') and name.endswith('.so')]
    assert len(extensions) == 1, extensions
    result = {
        'runtime_version': wheel_version(runtime),
        'runtime_split_sha256': hashlib.sha256(runtime.read('cozy_runtime/internal/execution/native_split.py')).hexdigest(),
        'tensorfs_version': wheel_version(tensorfs),
        'extension_sha256': hashlib.sha256(tensorfs.read(extensions[0])).hexdigest(),
        'shim_sha256': hashlib.sha256(tensorfs.read('tensorfs/_varena_torch.so')).hexdigest(),
        'gpu_uuid': gpu_uuid,
        'seed': 13817,
    }
output.write_text(json.dumps(result, indent=2) + '\n')
print(output)
