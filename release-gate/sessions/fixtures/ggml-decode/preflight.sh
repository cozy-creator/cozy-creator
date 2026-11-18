# preflight.sh, sourced by the sessions that run a drive package on a pod (ggml-decode, ggml-kernels).
# drive_check <drive dir> <module> [label=<uv.lock>]...: on the runner, before anything is rented (the $0 pre-flight runs it
# too), the candidates ($RUNTIME, $TENSORFS) stand in for the drive's lock and for each named lock by TensorD's rule
# (stands.py), and the drive's environment, built from its lock with the candidates over it (CPU Torch), imports <module>.
# The lock is a copy's: `cozy run` writes the drive's own from the RC's builds, which uv alone can't find, so this one takes
# the candidates' folders as find-links. Its TensorFS still comes from PyPI, as cozy's lock's does, so the rule judges the
# same pair.
STANDS=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/stands.py
drive_check() {
  local drive=$1 module=$2 name=${1##*/} pair said imported
  local check=$OUT/check-$name env=$OUT/check-env
  shift 2
  rm -rf "$check" "$env" && cp -r "$drive" "$check" && rm -f "$check/uv.lock"
  (cd "$check" && uv lock -q --find-links "$(dirname "$RUNTIME")" --find-links "$(dirname "$TENSORFS")") ||
    fail "$name's lock"
  for pair in "$name=$check/uv.lock" "$@"; do
    said=$(uv run -q --no-project --with packaging python "$STANDS" "${pair#*=}" "${pair%%=*}" "$RUNTIME" "$TENSORFS") ||
      fail "$said"
    say "$said"
  done
  uv export -q --frozen --no-hashes --no-emit-project --no-emit-package cozy-runtime --no-emit-package tensorfs \
    --project "$check" -o "$OUT/check-req.txt" || fail "export $name's lock"
  uv venv -q -p 3.12 "$env" && uv pip install -q -p "$env" --torch-backend cpu -r "$OUT/check-req.txt" &&
    uv pip install -q -p "$env" --no-deps "$RUNTIME" "$TENSORFS" "$check" || fail "$name's environment doesn't install"
  imported=$("$env/bin/python" -c "import $module, cozy_runtime, tensorfs
print('cozy-runtime', cozy_runtime.__version__, '· tensorfs', tensorfs.__version__)" 2>&1) ||
    fail "$name doesn't import: $(tail -c 400 <<<"$imported")"
  say "$name imports $module on the runner: $imported"
  rm -rf "$check" "$env"
}

# drive_lock <drive dir>: the drive locked as the pod's run will lock it. Calcifer locks local code when it stages it
# (crates/calcifer/src/local_code.rs `locked`: the open RC's Runtime build, the RC's TensorFS beside it) and keeps that lock for
# the copy, so installing the drive on the runner's own machine makes the lock the session's runs take. One uv can't make
# fails here, at $0, in uv's words; a runner with no machine of its own says so and goes on.
drive_lock() {
  local drive=$1 name=${1##*/}
  if (cd "$RUN_PROJECT" && cozy package install "$drive" --on local -y) > "$OUT/lock-$name.txt" 2>&1; then
    say "$name: locked and installed on the runner as a run stages it: $(grep -m1 -oE 'is installed on [^ ]+' "$OUT/lock-$name.txt")"
  elif grep -qiE 'lock_failed|unsatisfiable|no solution found' "$OUT/lock-$name.txt"; then
    tail -c 2500 "$OUT/lock-$name.txt" | tee -a "$R"
    fail "$name: its lock can't be made (lines above)"
  else
    say "$name: no lock check on the runner: $(tail -n 4 "$OUT/lock-$name.txt" | tr '\n' ' ' | cut -c1-400)"
  fi
}
