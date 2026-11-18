# preflight.sh, sourced by the sessions that run a drive package on a pod (ggml-decode, ggml-kernels).
# drive_check <drive dir> <module> [label=<uv.lock>]...: on the runner, before anything is rented (the $0 pre-flight runs it
# too), the candidates ($RUNTIME, $TENSORFS) stand in for each named lock by TensorD's rule (stands.py), and the drive's
# environment (CPU Torch) with the candidates in it, as the pod's holds them over its lock, imports <module>. The candidates go
# to uv by path: the RC's Runtime floors on a dev TensorFS (xs-216), which uv takes for a dependency's dependency only when
# it is handed one. The drive's own lock is Calcifer's to make (drive_lock).
STANDS=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/stands.py
drive_check() {
  local drive=$1 module=$2 name=${1##*/} pair said imported
  local check=$OUT/check-$name env=$OUT/check-env
  shift 2
  for pair in "$@"; do
    said=$(uv run -q --no-project --with packaging python "$STANDS" "${pair#*=}" "${pair%%=*}" "$RUNTIME" "$TENSORFS") ||
      fail "$said"
    say "$said"
  done
  rm -rf "$check" "$env" && cp -r "$drive" "$check" && rm -f "$check/uv.lock"
  uv venv -q -p 3.12 "$env" && uv pip install -q -p "$env" --torch-backend cpu "$check" "$RUNTIME" "$TENSORFS" ||
    fail "$name's environment doesn't install"
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

# on_candidates <step>: the step ran on the candidate wheels: TensorD said no fallback to its lock's pair
# (environment.rs's warnings). Its lines go into the report unedited.
on_candidates() {
  local said
  said=$(grep -E "it runs on (that series|that|its lock's)|wheel carries no kernels" "$OUT/$1.txt") || return 0
  tee -a "$R" <<<"$said"
  fail "$1 did not run on the candidate wheels (lines above)"
}

# pod_install <step> <package dir>: a session's first cells on its pod: the package staged there as its runs will stage it
# (locked by Calcifer, its environment built, the candidates over the lock), before any minutes go to a download or a build.
# A lock uv can't make, or a fallback to the lock's pair, ends the session here with the lines in the report.
pod_install() {
  local n=$1 dir back=$PWD code
  dir=$(cd "$2" && pwd) || fail "$n: no $2"
  cd "$RUN_PROJECT" || fail "$n: no project"
  step "$n" package install "$dir" --on "$NAME" -y; code=$?
  cd "$back" || fail "$n: no $back"
  [ "$code" = 0 ] || { tail -c 2500 "$OUT/$n.txt" | tee -a "$R"; fail "$n: ${dir##*/} doesn't install on $NAME (lines above)"; }
  on_candidates "$n"
  say "$n: ${dir##*/} is staged on $NAME on the candidates"
}
