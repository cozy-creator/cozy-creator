"""Stage immutable fixture source and its tiny GIL gate; no SDK/GPU installation."""
import argparse
import hashlib
import io
import json
import re
import subprocess
import tarfile
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--repo', type=Path, required=True)
parser.add_argument('--out', type=Path, required=True)
parser.add_argument('--python', type=Path, required=True)
args = parser.parse_args()
args.out.mkdir(parents=True, exist_ok=False)
assert not subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=no'], cwd=args.repo)
sha = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=args.repo).decode().strip()
source = 'release-gate/suites/varena/owner-runtime'
archive = subprocess.check_output(['git', 'archive', sha, source], cwd=args.repo)
with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
    tar.extractall(args.out, filter='data')
fixture = args.out / source
module = fixture / 'ci_varena_owner'
library = module / '_gil_gate.so'
command = ['nice', '-n', '19', 'cc', '-shared', '-fPIC', '-O2', '-Wl,--build-id=none',
           str(module / 'gil_gate.c'), '-o', str(library)]
subprocess.run(command, check=True)
elf = subprocess.check_output(['readelf', '--version-info', str(library)], text=True)
versions = set(re.findall(r'GLIBC_([0-9]+(?:\.[0-9]+)+)', elf))
assert versions and max(tuple(map(int, value.split('.'))) for value in versions) <= (2, 28)
code = '''import ctypes,json,sys
from pathlib import Path
library=ctypes.PyDLL(sys.argv[1])
library.hold_gil.argtypes=[]
library.hold_gil.restype=ctypes.c_int
result=library.hold_gil()
assert result==0, result
assert 'libcuda.so' not in Path('/proc/self/maps').read_text()
print(json.dumps({'returned':result,'cuda_loaded':False}),flush=True)
'''
child = subprocess.Popen([str(args.python), '-I', '-B', '-c', code, str(library)],
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
try:
    ready = json.loads(child.stdout.readline())
    assert ready == {'gil_gate': True, 'gil_held': 1}, ready
    child.stdin.write(b'x'); child.stdin.flush()
    returned = json.loads(child.stdout.readline())
    child.wait(timeout=5)
    assert child.returncode == 0, child.stderr.read().decode()
finally:
    if child.poll() is None:
        child.kill(); child.wait(timeout=5)
files = {str(p.relative_to(fixture)): hashlib.sha256(p.read_bytes()).hexdigest()
         for p in fixture.rglob('*') if p.is_file()}
report = {'source_commit': sha, 'fixture': str(fixture), 'files': files, 'build_command': command,
          'gil_gate_sha256': files['ci_varena_owner/_gil_gate.so'], 'glibc_versions': sorted(versions),
          'gil_gate_cpu': {'ready': ready, 'returned': returned}, 'gpu_qualification': False}
(args.out / 'fixture.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
