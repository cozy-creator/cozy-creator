"""Read exact cohort identity from wheels; keep payload outside frozen source."""
import hashlib
import json
import sys
import zipfile
from email.parser import BytesParser
from pathlib import Path

runtime, tensorfs, uuid, destination = sys.argv[1:]
out = Path(destination).resolve()
root = Path(__file__).resolve().parent
if out == root or root in out.parents:
    raise SystemExit('write payload outside frozen source')


def opened(path):
    archive = zipfile.ZipFile(path)
    metadata = next(n for n in archive.namelist() if n.endswith('.dist-info/METADATA'))
    return archive, BytesParser().parsebytes(archive.read(metadata))['Version']


def digest(archive, name):
    return hashlib.sha256(archive.read(name)).hexdigest()


r, rv = opened(runtime)
t, tv = opened(tensorfs)
payload = {'runtime_version': rv,
           'residency_sha256': digest(r, 'cozy_runtime/internal/residency.py'),
           'tensorfs_version': tv,
           'extension_sha256': digest(t, 'tensorfs/_ext.abi3.so'),
           'shim_sha256': digest(t, 'tensorfs/_varena_torch.so'), 'gpu_uuid': uuid}
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(payload, indent=2) + '\n')
print(out)
