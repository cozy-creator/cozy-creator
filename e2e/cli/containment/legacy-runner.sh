#!/usr/bin/env bash
# Coordinator for a real released writer followed by the candidate, on one fresh owned runner.
set -euo pipefail
privacy_stage=10
privacy_fail() {
  printf 'legacy_bootstrap_failed stage=%s\n' "$privacy_stage" >&2
  exit "${1:-2}"
}
trap 'privacy_fail "$?"' ERR
here=$(cd "$(dirname "$0")" && pwd)
[[ ${GITHUB_ACTIONS:-} == true && ${RUNNER_OS:-} == Linux && ${RUNNER_ENVIRONMENT:-} == github-hosted ]] || privacy_fail
privacy_stage=20
privacy_hub= privacy_candidates= privacy_out=
while [ $# -gt 0 ]; do
  [[ $# -ge 2 ]] || privacy_fail
  case $1 in
    --hub-bin) privacy_hub=$(realpath "$2");;
    --candidates) privacy_candidates=$2;;
    --out) privacy_out=$2;;
    *) privacy_fail;;
  esac
  shift 2
done
[[ -n $privacy_hub && -n $privacy_candidates && -n $privacy_out ]] || privacy_fail
privacy_stage=30
privacy_candidate= privacy_runtime= privacy_tensorfs=
read -r -a privacy_args <<< "$privacy_candidates"
for ((i=0;i+1<${#privacy_args[@]};i+=2)); do
  case ${privacy_args[i]} in
    --tensord) privacy_candidate=$(realpath "${privacy_args[i+1]}");;
    --runtime) privacy_runtime=$(realpath "${privacy_args[i+1]}");;
    --tensorfs) privacy_tensorfs=$(realpath "${privacy_args[i+1]}");;
  esac
done
privacy_stage=40
[[ -x $privacy_candidate && -f $privacy_runtime && -f $privacy_tensorfs ]] || privacy_fail
privacy_stage=50
mkdir -p "$privacy_out" "$RUNNER_TEMP/containment-released-client"
privacy_release=$(realpath "$RUNNER_TEMP/containment-released-client")
privacy_archive=cozy-x86_64-unknown-linux-gnu.tar.gz
privacy_url=https://github.com/cozy-creator/cozy-creator/releases/download/v0.24.0
privacy_stage=60
curl -fsSL --retry 3 "$privacy_url/$privacy_archive" -o "$privacy_release/$privacy_archive"
privacy_stage=70
curl -fsSL --retry 3 "$privacy_url/$privacy_archive.sha256" -o "$privacy_release/$privacy_archive.sha256"
privacy_stage=80
python3 - "$privacy_release/$privacy_archive" "$privacy_release/$privacy_archive.sha256" <<'PY'
import hashlib,re,sys
from pathlib import Path
archive, check = map(Path,sys.argv[1:])
expected=check.read_text().split()[0]
assert re.fullmatch(r'[0-9a-fA-F]{64}',expected)
with archive.open('rb') as source:
 assert hashlib.file_digest(source,'sha256').hexdigest()==expected.lower()
PY
privacy_stage=90
mkdir -p "$privacy_release/extracted"
tar -xzf "$privacy_release/$privacy_archive" -C "$privacy_release/extracted" --no-same-owner
privacy_stage=100
# Hosted runners do not promise ripgrep. The extracted public executable is a
# filesystem lookup, and find is already required by the outer fixture runner.
privacy_old=$(find "$privacy_release/extracted" -type f -name cozy -print -quit)
[[ -n $privacy_old ]] || privacy_fail
chmod +x "$privacy_old"
privacy_stage=110
privacy_ready=$(realpath "$privacy_out")/legacy-ready.json
python3 - "$privacy_candidate" "$privacy_ready" <<'PY'
import hashlib,json,sys
from pathlib import Path
candidate,ready=map(Path,sys.argv[1:])
with candidate.open('rb') as source:
 ready.write_text(json.dumps({'ready':True,'sha256':hashlib.file_digest(source,'sha256').hexdigest()}))
PY
# The OLD binary owns login/publication/V7 creation; the candidate is only a later argument.
privacy_stage=120
printf 'legacy_bootstrap_ready\n'
exec bash "$here/../../skew/run.sh" --hub-bin "$privacy_hub" --cozy-bin "$privacy_old" \
  --runtime "$privacy_runtime" --tensorfs "$privacy_tensorfs" --admin --private-artifacts \
  --out "$privacy_out/hub" --command "$here/legacy.sh" -- \
  --candidate "$privacy_candidate" --ready-file "$privacy_ready"
