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

# pkg_held <machine> <package dir>: the Runtime in the environment <machine> holds for the package and what its runs are told of
# the pair it holds (`cozy package ls`: a fallback and why), tab-separated; nothing when the listing names no such package.
pkg_held() {
  local name
  name=$(sed -n 's/^name = "\(.*\)"$/\1/p' "$2/pyproject.toml" | head -1)
  (cd "$RUN_PROJECT" && cozy --json package ls --on "$1") 2> /dev/null |
    jq -r --arg p "/$name" '.packages[]? | select(.package | endswith($p)) | [.runtime, .warning] | @tsv' 2> /dev/null | head -1
}

# drive_lock <drive dir>: the drive locked as the pod's run will lock it. Calcifer locks local code when it stages it
# (crates/calcifer/src/local_code.rs `locked`: the open RC's Runtime build, the RC's TensorFS beside it) and keeps that lock for
# the copy, so installing the drive on the runner's own machine makes the lock the session's runs take. One uv can't make
# fails here, at $0, in uv's words; a runner with no machine of its own says so and goes on.
drive_lock() {
  local drive=$1 name=${1##*/} took told said
  if (cd "$RUN_PROJECT" && cozy package install "$drive" --on local -y) > "$OUT/lock-$name.txt" 2>&1; then
    say "$name: locked and installed on the runner as a run stages it: $(grep -m1 -oE 'is installed on [^ ]+' "$OUT/lock-$name.txt")"
    # The Runtime that lock took (the runner has no pair of its own, so its environment holds the lock's), judged by the
    # stand-in rule: `cozy package install` writes no lock into the folder, as `cozy run` does.
    IFS=$'\t' read -r took told <<<"$(pkg_held local "$drive")"
    if [[ $took =~ ^[0-9]+\.[0-9]+ ]]; then
      printf 'name = "cozy-runtime"\nversion = "%s"\n' "$took" > "$OUT/lock-$name.pair"
      said=$(uv run -q --no-project --with packaging python "$STANDS" "$OUT/lock-$name.pair" "$name's lock on the runner" "$RUNTIME" "$TENSORFS") ||
        fail "$said"
      say "$said${told:+ · noted there: $told}"
    else
      say "$name: the runner's listing gives its environment's Runtime as '${took:-unlisted}': the lock's Runtime unjudged here"
    fi
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
  # Its environment holds the candidate Runtime: the wheel's commit in what the pod lists for it. Another build's commit
  # there is the lock's pair, and ends the session; a listing that names no commit is told and left to on_candidates.
  local took told sha
  IFS=$'\t' read -r took told <<<"$(pkg_held "$NAME" "$dir")"
  sha=$(grep -oE 'g[0-9a-f]{8}' <<<"${RUNTIME##*/}" | head -1)
  if grep -qE "it runs on (that series|that|its lock's)" <<<"$told" ||
    { [ -n "$sha" ] && [[ $took != *"$sha"* ]] && [[ $took =~ g[0-9a-f]{8} ]]; }; then
    say "$n: ${dir##*/}'s environment on $NAME holds cozy-runtime $took, not the candidate ${RUNTIME##*/}${told:+: $told}"
    fail "$n did not take the candidate wheels (line above)"
  fi
  say "$n: ${dir##*/} is staged on $NAME, its environment's Runtime ${took:-unlisted}${told:+ · $told}"
}
# The card a ggml session rents. params: `skus`, the SKUs it accepts, comma-separated (default the dispatch's $SKU), and
# `quoted`, the micro-USD/h the cheapest of them quoted at dispatch. pick_offer, just before `quote`, makes $SKU the cheapest
# of them offered now, and refuses when that is above 1.5x `quoted`; offer_rent, in place of `rent`, ends an order whose rate
# is above that before it boots (lalaru, 44400509329: a $1.511/h 4070 Ti Super where $0.51 was quoted at dispatch and 4090s
# offered $0.77 to $1.08).
OFFER_MAX=''
pick_offer() {
  local s f price best='' low=0 said='' quoted
  quoted=$(param quoted '')
  IFS=, read -ra offers <<<"$(param skus "$SKU")"
  # Out of stock comes and goes within minutes: every card asked again, six times over about 6 min, as `quote` asks one
  # (44447068529 asked once: "No provider has a40 right now … last offered at 02:49:58", while others rented).
  local try=1 pause=15
  while :; do
    said=''
    for s in "${offers[@]}"; do
      f=$OUT/offer-$s.json
      cozy machine rent "$s" --disk-gb "$DISK" $RENT_ARGS --json < /dev/null > "$f" 2> "${f%.json}.err" || true
      price=$(jq -R -r 'fromjson? | select(.quote) | .quote.usd_micros_per_hour' "$f" | head -1)
      said="$said $s ${price:-none}"
      [[ $price =~ ^[0-9]+$ ]] || continue
      [ -n "$best" ] && [ "$price" -ge "$low" ] || { best=$s low=$price; }
    done
    [ -z "$best" ] && [ "$try" -lt 6 ] || break
    say "offers: none for$said (attempt $try/6): asking again in ${pause}s"
    sleep "$pause"; try=$((try + 1)); pause=$((pause * 2 > 120 ? 120 : pause * 2))
  done
  [ -n "$best" ] || fail "offers: none for$said"
  say "offers$said micro-USD/h ($DISK GB): $best is the cheapest${quoted:+; $quoted quoted at dispatch}"
  if [ -n "$quoted" ]; then
    OFFER_MAX=$((quoted * 3 / 2))
    [ "$low" -le "$OFFER_MAX" ] || fail "offers: the cheapest, $best at $low, is above 1.5x the $quoted quoted at dispatch"
  fi
  SKU=$best
}
offer_rent() {
  local got
  order
  got=$(cozy machine show "$RID" --json 2> /dev/null | jq -r '.rental.usd_micros_per_hour // empty')
  say "$RID: $SKU at ${got:-?} micro-USD/h"
  [ -z "$OFFER_MAX" ] || [ -z "$got" ] || [ "$got" -le "$OFFER_MAX" ] ||
    fail "$RID rents at $got micro-USD/h, above 1.5x the quoted: ended before it boots"
  ready
}
