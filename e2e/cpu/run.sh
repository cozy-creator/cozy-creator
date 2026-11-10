#!/usr/bin/env bash
# e2e/cpu/run.sh: every package's entry points through the Runtime's real executor on the CPU, with tiny random
# weights (README "cpu"). No GPU, no rental, no sign-in.
#
#   e2e/cpu/run.sh --out DIR --packages DIR --runtime R --tensorfs T [--cozy BIN --project DIR] [--envs DIR]
#                  [--only pkg,...] [--cells entry/cell,...] [--jobs N] [--refs DIR] [--helpers pkg,...] [--record] [--keep]
# R and T: a wheel, a source dir (the Runtime), an exact release version, or `lock` (what each package's lock pins).
# --cozy: a cozy whose Calcifer is up, and --project a project folder of it: each package's code is also uploaded,
# locked and its cells' requests bound through it (cli.sh); without it those cells are not run.
# --keep keeps each package's tiny weights, inputs and outputs (else only logs and replies stay);
# --refs reads (and --record writes) references elsewhere than refs/; --helpers installs those packages from the
# checkout over the releases the locks pin (a helper not yet published). e2e/lib.sh's other candidate flags are ignored.
# A lock whose helper comes from the Hub's index, which needs a sign-in, falls back to the checkout's helper and says so.
#
# One line per cell: PASS|FAIL|SKIP <package> <entry> <cell> <seconds>s[: first error line]; the summary last.
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
OUT='' PACKAGES='' RUNTIME='' TENSORFS='' ENVS=${XDG_CACHE_HOME:-$HOME/.cache}/cozy-e2e-cpu ONLY='' CELLS='' JOBS=2 RECORD='' REFS='' HELPERS='' KEEP='' COZY='' PROJECT=''
while [ $# -gt 0 ]; do
  case $1 in
    --out) OUT=$2 ;; --packages) PACKAGES=$2 ;; --runtime) RUNTIME=$2 ;; --tensorfs) TENSORFS=$2 ;;
    --envs) ENVS=$2 ;; --cozy) COZY=$2 ;; --project) PROJECT=$2 ;; --only) ONLY=$2 ;; --cells) CELLS=$2 ;; --jobs) JOBS=$2 ;; --refs) REFS=$2 ;; --helpers) HELPERS=$2 ;;
    --record) RECORD=--record; shift; continue ;;
    --keep) KEEP=1; shift; continue ;;
    --tensord | --pod-tensord | --release) ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
  shift 2
done
[ -n "$OUT" ] && [ -d "$PACKAGES" ] && [ -n "$RUNTIME" ] && [ -n "$TENSORFS" ] \
  || { echo "usage: run.sh --out DIR --packages DIR --runtime R --tensorfs T" >&2; exit 2; }
[ -z "$COZY" ] || [ -n "$PROJECT" ] || { echo "--cozy needs --project" >&2; exit 2; }
abs() { if [ -e "$1" ]; then (cd "$(dirname "$1")" && echo "$PWD/$(basename "$1")"); else echo "$1"; fi; }
OUT=$(mkdir -p "$OUT" && cd "$OUT" && pwd) PACKAGES=$(cd "$PACKAGES" && pwd) RUNTIME=$(abs "$RUNTIME") TENSORFS=$(abs "$TENSORFS")
mkdir -p "$ENVS"
REFS=$(mkdir -p "${REFS:-$HERE/refs}" && cd "${REFS:-$HERE/refs}" && pwd)
export PACKAGES RUNTIME TENSORFS ENVS OUT RECORD CELLS HERE REFS HELPERS KEEP COZY PROJECT

# A package's fixture is tiny/<its name, - and . as _>.py. Every package in the checkout runs, or --only's.
fixture() { local f=${1//-/_}; echo "$HERE/tiny/${f//./_}.py"; }
export -f fixture
if [ -n "$ONLY" ]; then packages=${ONLY//,/ }
else packages=$(cd "$PACKAGES" && for d in */pyproject.toml; do dirname "$d"; done); fi
# The slowest first, so the jobs end together: H3's cells are a third of the whole.
SLOWEST="minimax-h3 qwen-image-2 hunyuan-image-3"
packages=$(echo $packages)
packages="$(for q in $SLOWEST; do [[ " $packages " != *" $q "* ]] || echo "$q"; done) $(for q in $packages; do [[ " $SLOWEST " == *" $q "* ]] || echo "$q"; done)"

one() {
  local p=$1 out=$OUT/$1 env=$ENVS/$1 t
  rm -rf "$out"; mkdir -p "$out"
  if [ ! -f "$(fixture "$p")" ]; then echo "FAIL $p - fixture 0s: no tiny/$(basename "$(fixture "$p")")"; return 1; fi
  t=$SECONDS
  local helpers=() h
  for h in ${HELPERS//,/ }; do grep -q "^name = \"$h\"$" "$PACKAGES/$p/uv.lock" && helpers+=("$PACKAGES/$h"); done
  local made=1
  "$HERE/env.sh" "$PACKAGES/$p" "$env" "$RUNTIME" "$TENSORFS" "${helpers[@]}" > "$out/env.log" 2>&1 || made=''
  if [ -z "$made" ] && grep -qE '401|403|Unauthorized|Forbidden|authenticat' "$out/env.log"; then
    # The Hub's index refused an anonymous read: its helpers from the checkout, the lock's own unchecked.
    helpers=()
    for h in $(grep -B2 'source = { registry = "https://tensorhub.com' "$PACKAGES/$p/uv.lock" | sed -n 's/^name = "\(.*\)"/\1/p'); do
      [ -d "$PACKAGES/$h" ] && helpers+=("$PACKAGES/$h")
    done
    echo "SKIP $p - locked-helpers 0s: the Hub's index needs a sign-in; ran with the checkout's ${helpers[*]##*/}"
    made=1
    "$HERE/env.sh" "$PACKAGES/$p" "$env" "$RUNTIME" "$TENSORFS" "${helpers[@]}" > "$out/env.log" 2>&1 || made=''
  fi
  if [ -z "$made" ]; then
    echo "FAIL $p - environment $((SECONDS - t))s: $(grep -m1 -iE 'error|failed' "$out/env.log" || tail -n1 "$out/env.log")"
    return 1
  fi
  echo "PASS $p - environment $((SECONDS - t))s"
  # The driver and the package's fixture, strictly typed against the package's own environment, beside its cells (one
  # core for a minute, cold).
  (s=$SECONDS; code=0
   nice -n 19 uvx -q mypy@2.4.1 --config-file "$HERE/mypy.ini" --cache-dir "$env/.mypy" --python-executable "$env/bin/python" \
     "$HERE/drive.py" "$HERE/cell.py" "$HERE/surface.py" "$HERE/waive/cpu_e2e_waive.py" "$(fixture "$p")" > "$out/mypy.txt" 2>&1 || code=1
   echo "$code $((SECONDS - s))" > "$out/mypy.rc") &
  local typing=$!
  typed() {
    wait "$typing"
    local code s; read -r code s < "$out/mypy.rc"
    if [ "$code" = 0 ]; then echo "PASS $p - typecheck ${s}s"
    else echo "FAIL $p - typecheck ${s}s: $(grep -m1 'error:' "$out/mypy.txt" || tail -n1 "$out/mypy.txt")"; fi
  }
  t=$SECONDS
  # The static reader's interface: since Runtime 0.44 (cr-269) the only describer, so no import to compare.
  if ! CUDA_VISIBLE_DEVICES= nice -n 19 "$env/bin/cozy-runtime" --json describe --dir "$PACKAGES/$p" \
      > "$out/interface.json" 2> "$out/describe.err"; then
    local why; why=$(grep -o '"message": *"[^"]*' "$out/describe.err" | head -n1 | sed 's/"message": *"//')
    echo "FAIL $p - describe $((SECONDS - t))s: $(echo "${why:-$(grep -v '^\s*$' "$out/describe.err" | tail -n1)}" | cut -c1-300)"
    typed
    return 1
  fi
  echo "PASS $p - describe $((SECONDS - t))s"
  CUDA_VISIBLE_DEVICES= nice -n 19 "$env/bin/python" "$HERE/drive.py" "$PACKAGES/$p" "$out/interface.json" \
    "$(fixture "$p")" "$out" --refs "$REFS" ${CELLS:+--only "$CELLS"} $RECORD 2> "$out/drive.err" || true
  [ -z "$COZY" ] || "$HERE/cli.sh" "$COZY" "$PROJECT" "$PACKAGES/$p" "$out"
  typed
  [ -n "$KEEP" ] || rm -rf "$out"/{store,source,spool,made,inputs,cli/src}
}
export -f one

started=$SECONDS
printf '%s\n' $packages | xargs -P "$JOBS" -I{} bash -c 'one "$@"' _ {} | tee "$OUT/cells.txt" || true
# Static: the TensorFS names the Runtime reads, against the candidate TensorFS (in any package's environment).
for q in $packages; do
  py=$ENVS/$q/bin/python
  [ -x "$py" ] || continue
  CUDA_VISIBLE_DEVICES= "$py" "$HERE/surface.py" "$("$py" -c 'import cozy_runtime, os; print(os.path.dirname(cozy_runtime.__file__))')" \
    | tee -a "$OUT/cells.txt" || true
  break
done
pass=$(grep -c '^PASS' "$OUT/cells.txt" || true) fail=$(grep -c '^FAIL' "$OUT/cells.txt" || true)
echo "cpu: $pass passed, $fail failed in $((SECONDS - started)) s"
[ "$fail" = 0 ] && [ "$pass" != 0 ]
