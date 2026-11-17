# The differential arms' helpers (cell.sh, suite graphs): a package copy with memoization out and hook.py in, its runs,
# and two records judged as one output (differential.py). Sourced: needs $HERE, $OUT, $PKGS, run_cell, run_get, expect, log.
D=$HERE/suites/varena/differential
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
