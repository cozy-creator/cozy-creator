#!/usr/bin/env bash
# e2e/skew/run.sh --cozy-bin RELEASED_0_24_0 --admin --command this --
#   --candidate NEW_COZY --ready-file READY.json
# The outer runner must select the old CLI BEFORE login/publication; --candidates
# there would open V8 before this real released-writer subject exists.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=$1 OLD=$2 RUNTIME=$3 TENSORFS=$4
shift 4
[[ ${GITHUB_ACTIONS:-} == true && ${RUNNER_OS:-} == Linux && ${RUNNER_ENVIRONMENT:-} == github-hosted ]] || exit 2
candidate= ready= released=
while [ $# -gt 0 ]; do
  case $1 in
    --candidate) candidate=$(realpath "$2");;
    --ready-file) ready=$(realpath "$2");;
    --released) released=$(realpath "$2");;
    *) exit 2;;
  esac
  shift 2
done
[[ -x $candidate && -f $ready && -x $OLD && -x $released ]] || exit 2
# Public dependencies are resolved before any private canary exists. Private
# commands below never use e2e's argv.log/raw NDJSON/service-capture wrappers.
exec uv run --no-project --with cryptography --with h2 --with python-snappy --with pillow \
  python "$HERE/legacy.py" --old "$OLD" --candidate "$candidate" --ready-file "$ready" \
  --released "$released" --out "$OUT" --runtime "$RUNTIME" --tensorfs "$TENSORFS"
