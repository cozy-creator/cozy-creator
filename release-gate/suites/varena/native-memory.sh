#!/usr/bin/env bash
# Build against this candidate; optionally run on an explicitly selected local CUDA ordinal.
# Usage: native-memory.sh OUTPUT_DIRECTORY [CUDA_ORDINAL]
set -euo pipefail
if (( $# < 1 || $# > 2 )); then
  echo 'usage: native-memory.sh OUTPUT_DIRECTORY [CUDA_ORDINAL]' >&2
  exit 2
fi
probe_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
probe_repo=$(git -C "$probe_dir" rev-parse --show-toplevel)
mkdir -p -- "$1"
probe_out=$(cd -- "$1" && pwd)
nice -n 19 cargo build --manifest-path "$probe_repo/Cargo.toml" --locked -j 2 -p varena --message-format=json > "$probe_out/cargo-build.jsonl"
nice -n 19 python3 - "$probe_repo" "$probe_out" <<'PY'
import hashlib, json, os, pathlib, subprocess, sys
repo, out = map(pathlib.Path, sys.argv[1:])
artifacts = {}
for line in (out / 'cargo-build.jsonl').read_text().splitlines():
    message = json.loads(line)
    if message.get('reason') != 'compiler-artifact':
        continue
    name = message['target']['name']
    if name in {'varena', 'tensorfs', 'serde_json'} and 'lib' in message['target']['kind']:
        artifacts.setdefault(name, set()).update(p for p in message['filenames'] if p.endswith('.rlib'))
for name in ['varena', 'tensorfs', 'serde_json']:
    if len(artifacts.get(name, set())) != 1:
        raise SystemExit(f'expected one exact candidate artifact for {name}: {artifacts.get(name)}')
source = repo / 'release-gate/suites/varena/native-memory.rs'
commit = subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'], text=True).strip()
dirty = bool(subprocess.check_output(['git', '-C', str(repo), 'status', '--porcelain', '--untracked-files=normal'], text=True).strip())
source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
env = dict(os.environ, NATIVE_MEMORY_SOURCE_COMMIT=commit, NATIVE_MEMORY_SOURCE_DIRTY=str(dirty).lower(), NATIVE_MEMORY_PROBE_SHA256=source_hash)
linked = {name: next(iter(paths)) for name, paths in artifacts.items()}
deps = pathlib.Path(linked['varena']).parent
binary = out / 'native-memory'
command = ['rustc', '--edition=2021', str(source), '-L', f'dependency={deps}']
for name, path in linked.items():
    command += ['--extern', f'{name}={path}']
command += ['-o', str(binary)]
subprocess.run(command, env=env, check=True)
(out / 'build.json').write_text(json.dumps({'commit': commit, 'dirty': dirty, 'probe_sha256': source_hash,
    'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'artifacts': linked}, indent=2) + '\n')
print(binary)
PY
if (( $# == 2 )); then
  "$probe_out/native-memory" "$2" | tee "$probe_out/native-memory.json"
fi
