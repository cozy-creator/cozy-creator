#!/usr/bin/env bash
# e2e/cpu/run.sh: every package's entry points through the Runtime's real executor on the CPU, with tiny random
# weights (README "cpu"). No GPU, no rental, no sign-in.
#
#   e2e/cpu/run.sh --out DIR --packages DIR --runtime <dir|wheel> --tensorfs <wheel> [--envs DIR] [--only pkg,...]
#                  [--cells entry/cell,...] [--jobs N] [--record]
#
# One line per cell: PASS|FAIL|SKIP <package> <entry> <cell> <seconds>s[: first error line]; the summary last.
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
OUT='' PACKAGES='' RUNTIME='' TENSORFS='' ENVS=${XDG_CACHE_HOME:-$HOME/.cache}/cozy-e2e-cpu ONLY='' CELLS='' JOBS=2 RECORD=''
while [ $# -gt 0 ]; do
  case $1 in
    --out) OUT=$2 ;; --packages) PACKAGES=$2 ;; --runtime) RUNTIME=$2 ;; --tensorfs) TENSORFS=$2 ;;
    --envs) ENVS=$2 ;; --only) ONLY=$2 ;; --cells) CELLS=$2 ;; --jobs) JOBS=$2 ;;
    --record) RECORD=--record; shift; continue ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
  shift 2
done
[ -n "$OUT" ] && [ -d "$PACKAGES" ] && [ -e "$RUNTIME" ] && [ -f "$TENSORFS" ] \
  || { echo "usage: run.sh --out DIR --packages DIR --runtime <dir|wheel> --tensorfs <wheel>" >&2; exit 2; }
abs() { (cd "$(dirname "$1")" && echo "$PWD/$(basename "$1")"); }
OUT=$(mkdir -p "$OUT" && cd "$OUT" && pwd) PACKAGES=$(cd "$PACKAGES" && pwd) RUNTIME=$(abs "$RUNTIME") TENSORFS=$(abs "$TENSORFS")
mkdir -p "$ENVS"
export PACKAGES RUNTIME TENSORFS ENVS OUT RECORD CELLS HERE

# A package's fixture is tiny/<its name, - and . as _>.py. Every package in the checkout runs, or --only's.
fixture() { local f=${1//-/_}; echo "$HERE/tiny/${f//./_}.py"; }
export -f fixture
if [ -n "$ONLY" ]; then packages=${ONLY//,/ }
else packages=$(cd "$PACKAGES" && for d in */pyproject.toml; do dirname "$d"; done); fi

one() {
  local p=$1 out=$OUT/$1 env=$ENVS/$1 t
  rm -rf "$out"; mkdir -p "$out"
  if [ ! -f "$(fixture "$p")" ]; then echo "FAIL $p - fixture 0s: no tiny/$(basename "$(fixture "$p")")"; return 1; fi
  t=$SECONDS
  if ! "$HERE/env.sh" "$PACKAGES/$p" "$env" "$RUNTIME" "$TENSORFS" > "$out/env.log" 2>&1; then
    echo "FAIL $p - environment $((SECONDS - t))s: $(grep -m1 -iE 'error|failed' "$out/env.log" || tail -n1 "$out/env.log")"
    return 1
  fi
  echo "PASS $p - environment $((SECONDS - t))s"
  t=$SECONDS
  # The driver and the package's fixture, strictly typed against the package's own environment.
  if ! nice -n 19 uvx -q mypy@2.4.1 --config-file "$HERE/mypy.ini" --cache-dir "$env/.mypy" --python-executable "$env/bin/python" \
      "$HERE/drive.py" "$HERE/cell.py" "$HERE/waive/sitecustomize.py" "$(fixture "$p")" > "$out/mypy.txt" 2>&1; then
    echo "FAIL $p - typecheck $((SECONDS - t))s: $(grep -m1 'error:' "$out/mypy.txt" || tail -n1 "$out/mypy.txt")"
  else
    echo "PASS $p - typecheck $((SECONDS - t))s"
  fi
  t=$SECONDS
  # The static reader's interface, refused unless the import agrees (`--conformance`).
  if ! CUDA_VISIBLE_DEVICES= nice -n 19 "$env/bin/cozy-runtime" --json describe --conformance --dir "$PACKAGES/$p" \
      > "$out/interface.json" 2> "$out/describe.err"; then
    echo "FAIL $p - conformance $((SECONDS - t))s: $(grep -v '^\s*$' "$out/describe.err" | tail -n1 | cut -c1-300)"
    return 1
  fi
  echo "PASS $p - conformance $((SECONDS - t))s"
  CUDA_VISIBLE_DEVICES= nice -n 19 "$env/bin/python" "$HERE/drive.py" "$PACKAGES/$p" "$out/interface.json" \
    "$(fixture "$p")" "$out" ${CELLS:+--only "$CELLS"} $RECORD 2> "$out/drive.err"
}
export -f one

started=$SECONDS
printf '%s\n' $packages | xargs -P "$JOBS" -I{} bash -c 'one "$@"' _ {} | tee "$OUT/cells.txt" || true
pass=$(grep -c '^PASS' "$OUT/cells.txt" || true) fail=$(grep -c '^FAIL' "$OUT/cells.txt" || true)
echo "cpu: $pass passed, $fail failed in $((SECONDS - started)) s"
[ "$fail" = 0 ] && [ "$pass" != 0 ]
