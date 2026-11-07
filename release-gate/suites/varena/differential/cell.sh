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
#   eager    graphs off, every loop and component root as written: byte-identical to base's first call, so a capture
#            that leaves work out (28892's class) shows here. Every first-party package that records a loop
# diff-writes: SDXL's step, `before` and `after` also bump counters outside the loop's tensors (a torch op, a raw Triton
#   launch), over a first call that records, then two that reuse it, and over a first call that records nothing, the idle
#   recording after it, and two that reuse that: each counter exactly what running without graphs gives (90721eb84 ran a
#   step and its `after` twice, and read 5 where 4 was due)
# At the same split levels the bytes must match. Where levels moved with the room, or `why` names a cause the bytes
# follow, the outputs are judged by quality (differential.py; Paul 11-06: determinism is desirable, not required).
# A skipped or unreadable arm is a FAIL: a gate that did not run this has not passed it.
D=$V/differential
ARMS=$OUT/differential
# diff_copy <package> <arm> <checks> [press]: the package with memoization out and hook.py in, at ./differential/<arm>/<package>;
# a press writes* adds writes.py.
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
  case ${4:-} in
    writes) cat "$D/writes.py" >> "$to";;
    writes-idle) sed 's/^IDLE = .*/IDLE = True/' "$D/writes.py" >> "$to";;
  esac
  grep -q "^ARM: dict\[str, bool\] = dict($3)" "$to"
}
diff_run() {  # diff_run <cell> <arm> <package> <function> <args...>: H3's text and length as one beat (gate.sh's one_beat)
  local name=$1 arm=$2 package=$3 function=$4; shift 4
  [ "$package/$function" != minimax-h3/generate ] || { one_beat "$@"; set -- "${BEAT[@]}"; }
  run_cell "$OUT" "$name" "./differential/$arm/$package/$function" --on "$M" "$@"
  log "$name run $(cat "$name.number" 2>/dev/null) $(run_get "$OUT" "$name" .status)"
}
# why <check>: what else the check's bytes follow, each seen in v1 0.20.1 on the same 5090 too (11-06).
why() {
  case $1 in
    diff-sdxl-capped) echo "capped SDXL's first and second calls differ with no split moved: a kernel choice follows free memory";;
    diff-sdxl-noroom) echo "its room differs by design: kernel choices and split levels follow it";;
    diff-h3-repeat|diff-h3-capped) echo "Sol serves a length's first sight dense while its sparse kernel opens: a first call is not a second";;
  esac
}
diff_expect() {  # diff_expect <check> <a> <b>
  local said; said=$(python3 "$D/differential.py" "$2.json" "$3.json" "$1" "$(why "$1")" 2>&1)
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
diff_eager() {  # diff_eager <name> <package> <function> <args...>: graphs off against base's first call
  local name=$1 package=$2 function=$3 native; shift 3
  [ -z "$ONLY" ] || [[ $ONLY == *,differential,* || $ONLY == *",diff-$name,"* ]] || return 0
  if [ ! -f "diff-$name-base-1.json" ]; then
    diff_copy "$package" base "" || { expect "diff-$name-eager" FAIL "no copy of $package with the hook in it"; return; }
    diff_run "diff-$name-base-1" base "$package" "$function" "$@"
  fi
  diff_copy "$package" eager "" eager || { expect "diff-$name-eager" FAIL "no copy of $package with the hook in it"; return; }
  diff_run "diff-$name-eager" eager "$package" "$function" "$@"
  native=$(jq '[.. | .native_steps? // 0] | add' "diff-$name-base-1.json" 2>/dev/null)
  log "diff-$name-eager: base's first call ran ${native:-no} steps natively"
  diff_expect "diff-$name-eager" "diff-$name-base-1" "diff-$name-eager"
}
diff_writes() {  # diff_writes <package> <function> <args...>
  local package=$1 function=$2 arm i said; shift 2
  [ -z "$ONLY" ] || [[ $ONLY == *,differential,* || $ONLY == *",diff-writes,"* ]] || return 0
  for arm in writes writes-idle; do
    diff_copy "$package" "$arm" "" "$arm" || { expect "diff-$arm" FAIL "no copy of $package with the hook in it"; continue; }
    for i in 1 2 3; do
      diff_run "diff-$arm-$i" "$arm" "$package" "$function" "$@"
      [ "$arm-$i" != writes-idle-1 ] || sleep 30  # its idle period records the call
    done
    said=$(python3 "$D/counts.py" "$arm" "diff-$arm-1.json" "diff-$arm-2.json" "diff-$arm-3.json" 2>&1)
    case $said in PASS*) expect "diff-$arm" PASS "${said#PASS }";; *) expect "diff-$arm" FAIL "${said#FAIL }";; esac
  done
}
if [ -z "$PKGS" ]; then
  [ -n "$ONLY" ] && [[ $ONLY != *,differential,* ]] || expect differential FAIL "needs --packages: nothing was run, so nothing is proven"
else
  mkdir -p "$ARMS"
  diff_model sdxl sdxl generate 6GiB uses prompt="$OBJECT on a wooden table, photo" seed:=$((R + 71))
  diff_press sdxl sdxl generate prompt="$OBJECT on a wooden table, photo" seed:=$((R + 71))
  diff_eager sdxl sdxl generate prompt="$OBJECT on a wooden table, photo" seed:=$((R + 71))
  diff_writes sdxl generate prompt="$OBJECT on a shelf, photo" seed:=$((R + 74))
  diff_model qwen qwen-image-2 generate 8GiB uses prompt="$SCENE, photo" seed:=$((R + 72)) megapixels:=4
  diff_eager qwen qwen-image-2 generate prompt="$SCENE, photo" seed:=$((R + 72)) megapixels:=4
  diff_eager krea krea-2 generate prompt="$SCENE, photo" seed:=$((R + 75)) steps:=8
  diff_eager moss moss-image generate prompt="$SCENE, photo" seed:=$((R + 76)) steps:=8
  diff_eager h3-full minimax-h3 generate prompt="$MOTION Wide, steady." seed:=$((R + 77)) duration_s:=5 turbo:=false steps:=30
  # Guards split no run, so H3's 2 GiB blocks find no room at 12 GB (31012, 31020): bigger cards keep them.
  [ "$(grep -oE '· [0-9]+ GiB ·' machine.txt | head -1 | grep -oE '[0-9]+')" -gt 12 ] || CHECKS="unmap_idle=True, perturb=True"
  diff_model h3 minimax-h3 generate 8GiB perturbed_bytes prompt="$MOTION Wide, steady." seed:=$((R + 73)) duration_s:=5 turbo:=true
fi
