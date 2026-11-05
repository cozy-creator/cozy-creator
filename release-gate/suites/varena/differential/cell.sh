# differential: a model must not know what its memory went through, and one request is one answer. Sourced by cells.sh.
# Each model runs the same request and seed in each arm, so Paul's unique-request rule doesn't apply: this is correctness,
# not a benchmark. Arms run in turn, because swapping a package's source ends its executors, and each second call is a
# real warm repeat: the copies have call memoization stripped, and hook.py records digests of latents and outputs.
#   repeat   base 1 against base 2, warm: byte-identical
#   checks   vt.checks on, every idle pool unmapped before each use, scratch pages, freed blocks and reused ones perturbed,
#            guards between large blocks: its calls against base's, byte for byte. `uses` must have reached a pool,
#            `perturbed_bytes` must have touched memory: H3 Turbo captures no graph, so it has no pool to reach
#   capped   twice under a cap, on a repeat: byte-identical
#   refill   a kept program's weights pushed off the card before its second call: its replay byte-identical to base's
#   noroom   they can't all come back: that call runs eagerly, byte-identical to base's replay, and says so
# A skipped or unreadable arm is a FAIL: a gate that did not run this has not passed it.
D=$V/differential
ARMS=$OUT/differential
# diff_copy <package> <arm> <checks> [press]: the package with memoization out and hook.py in, at ./differential/<arm>/<package>.
diff_copy() {
  local copy=$ARMS/$2/$1 module to
  rm -rf "$copy"; mkdir -p "$copy"
  tar -C "$PKGS/$1" --exclude=.venv --exclude=__pycache__ -cf - . | tar -C "$copy" -xf - || return 1
  grep -rlE --include='*.py' '^ *memoize=' "$copy" | xargs -r sed -i -E '/^ *memoize=(True|Memoize\(requires=\["seed"\]\)),$/d'
  module=$(sed -n 's/^object = "\([a-z0-9_]*\):.*/\1/p' "$copy/package.toml")
  if [ -d "$copy/$module" ]; then
    to=$copy/$module/gate_hook.py; printf '\nfrom . import gate_hook  # noqa: E402, F401\n' >> "$copy/$module/__init__.py"
  else
    [ -f "$copy/$module.py" ] || return 1
    to=$copy/gate_hook.py; printf '\nimport gate_hook  # noqa: E402, F401\n' >> "$copy/$module.py"
  fi
  sed -e "s/^ARM: .*/ARM: dict[str, bool] = dict($3)/" -e "s/^PRESS = .*/PRESS = \"${4:-}\"/" "$D/hook.py" > "$to"
  grep -q "^ARM: dict\[str, bool\] = dict($3)" "$to"
}
diff_run() {  # diff_run <cell> <arm> <package> <function> <args...>
  local name=$1 arm=$2 package=$3 function=$4; shift 4
  run_cell "$OUT" "$name" "./differential/$arm/$package/$function" --on "$M" "$@"
  log "$name run $(cat "$name.number" 2>/dev/null) $(run_get "$OUT" "$name" .status)"
}
diff_expect() {  # diff_expect <check> <a> <b>
  local said; said=$(python3 "$D/differential.py" "$2.json" "$3.json" 2>&1)
  case $said in PASS*) expect "$1" PASS "${said#PASS }";; *) expect "$1" FAIL "${said#FAIL }";; esac
}
diff_model() {  # diff_model <name> <package> <function> <cap> <uses|perturbed_bytes> <args...>
  local name=$1 package=$2 function=$3 limit=$4 reach=$5 i; shift 5
  [ -z "$ONLY" ] || [[ $ONLY == *,differential,* || $ONLY == *",diff-$name,"* ]] || return 0
  diff_copy "$package" base "" && diff_copy "$package" on "${CHECKS:-unmap_idle=True, perturb=True, guards=True}" ||
    { expect "diff-$name" FAIL "no copy of $package with the hook in it"; return; }
  for i in 1 2; do diff_run "diff-$name-base-$i" base "$package" "$function" "$@"; done
  for i in 1 2; do diff_run "diff-$name-on-$i" on "$package" "$function" "$@"; done
  diff_expect "diff-$name-repeat" "diff-$name-base-1" "diff-$name-base-2"
  for i in 1 2; do diff_expect "diff-$name-checks-$i" "diff-$name-base-$i" "diff-$name-on-$i"; done
  [ "$(jq --arg r "$reach" '[.. | .checks?[$r]? // 0] | max' "diff-$name-on-2.json" 2>/dev/null)" != 0 ] ||
    expect "diff-$name-reach" FAIL "the checks did nothing: $reach is 0"
  cap "$limit"
  for i in 1 2; do diff_run "diff-$name-cap-$i" base "$package" "$function" "$@"; done
  cap off
  diff_expect "diff-$name-capped" "diff-$name-cap-1" "diff-$name-cap-2"
}
diff_press() {  # diff_press <name> <package> <function> <args...>: after diff_model, against its base-2
  local name=$1 package=$2 function=$3 arm i; shift 3
  [ -z "$ONLY" ] || [[ $ONLY == *,differential,* || $ONLY == *",diff-$name,"* ]] || return 0
  for arm in refill hold; do
    diff_copy "$package" "$arm" "" "$arm" || { expect "diff-$name-$arm" FAIL "no copy of $package with the hook in it"; return; }
    for i in 1 2; do diff_run "diff-$name-$arm-$i" "$arm" "$package" "$function" "$@"; done
  done
  diff_expect "diff-$name-refill" "diff-$name-base-2" "diff-$name-refill-2"
  diff_expect "diff-$name-noroom" "diff-$name-base-2" "diff-$name-hold-2"
  [ "$(jq '[.. | .native_steps? // 0] | max' "diff-$name-refill-2.json" 2>/dev/null)" != 0 ] ||
    expect "diff-$name-refill-replayed" FAIL "the second call ran eagerly: nothing refilled for a replay"
  [ "$(jq '[.. | .weight_residency?.evictions? // 0] | max' "diff-$name-refill-2.json" 2>/dev/null)" != 0 ] ||
    expect "diff-$name-refill-left" FAIL "no weight left the card: the replay refilled nothing"
  [ "$(jq '[.. | .no_room? // empty] | length' "diff-$name-hold-2.json" 2>/dev/null)" != 0 ] ||
    expect "diff-$name-noroom-said" FAIL "the record says nothing of a program with no room"
}
if [ -z "$PKGS" ]; then
  [ -n "$ONLY" ] && [[ $ONLY != *,differential,* ]] || expect differential FAIL "needs --packages: nothing was run, so nothing is proven"
else
  mkdir -p "$ARMS"
  diff_model sdxl sdxl generate 6GiB uses prompt="$OBJECT on a wooden table, photo" seed:=$((R + 71))
  diff_press sdxl sdxl generate prompt="$OBJECT on a wooden table, photo" seed:=$((R + 71))
  diff_model qwen qwen-image-2 generate 8GiB uses prompt="$SCENE, photo" seed:=$((R + 72)) megapixels:=4
  # Guards split no run, so H3's 2 GiB blocks find no room at 12 GB (31012, 31020): bigger cards keep them.
  [ "$(grep -oE '· [0-9]+ GiB ·' machine.txt | head -1 | grep -oE '[0-9]+')" -gt 12 ] || CHECKS="unmap_idle=True, perturb=True"
  diff_model h3 minimax-h3 generate 8GiB perturbed_bytes prompt="$MOTION Wide, steady." seed:=$((R + 73)) duration_s:=5 turbo:=true
fi
