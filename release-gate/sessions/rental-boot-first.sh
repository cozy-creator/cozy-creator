# rental-boot-first (cl-1069). Escaped on the RC, 2026-11-16: LAB2-47826 (lab3_2) sent `install local/qwen-image-2`, whose
# `cozy-runtime>=0.47.0.dev0` only the open RC's Runtime meets, to the rental frosch as it booted on the 0.25.0 image's Runtime
# 0.46.0: refused `runtime_floor_unmet` at 07:04:14Z, and Calcifer moved frosch to the RC 2 s later (COZY-48658). Paul (11-16):
# "It should be a standing rule that we allow version skew"; "if the remote machine is out of date … when it starts up, it
# should check for an upgraded version and upgrade to that automatically"; "I should be able to send jobs and it just works.
# Either the remote machine or my local machine falls through and catches it."
# A dev home (`dev = true`) on the session's RC cozy (pod-session `cozy`: a dev-build run of rc/<x>), one rental of $SKU:
#   boot  local code with such a floor (fixtures/rc-floor) sent to a rental of $SKU as it boots from the release image, as
#         LAB2-47826's render sent it: it waits for the rental's move to the RC and completes on the RC's Runtime, never refused
#         `runtime_floor_unmet`. Before the fix this races the rental's first Status: it may pass by luck.
#   held  the rental put back by hand on the release image's Runtime and TensorFS (a hold), the same code `--on` it: refused
#         `runtime_held`, naming the update, before any install; before the fix the install went and was refused
#         `runtime_floor_unmet`.
# Every send here names its rental: the account's rentals are one pool (cl-1224), so a run naming none may go to any session's.
# A refusal no send foresaw (`Machines::refloored`) is reached only by a run naming no machine, so it is proven on stood-in
# rentals of a runner's own Hub (e2e/cli/rcmoves.sh), never here.
# Params: floor (the code's `cozy-runtime` requirement; default the open RC's Runtime minor, `>=0.47.0.dev0`), runtime and
# tensorfs (the release pair `held` puts back; default the 0.25.0 image's 0.46.0 and 0.25.1).
. "$(dirname "$0")/lib.sh"
check() { say "FAIL: $*"; echo "::error::$*"; RC=1; }
RUNTIME=$(param runtime 0.46.0) TENSORFS=$(param tensorfs 0.25.1) FLOOR=$(param floor)
v=$(cozy --version | awk '{print $2}')
[[ $v == *-dev* ]] || fail "cozy $v is no RC build: dispatch with cozy=<a dev-build run of rc/<x>>"
if [ -z "$FLOOR" ]; then
  w=$(gh release view "rc-${v%%-*}" -R cozy-creator/cozy-runtime --json assets -q '.assets[].name' | grep -oE '^cozy_runtime-[0-9]+\.[0-9]+\.[0-9]+\.dev[0-9]+' | head -1)
  [ -n "$w" ] || fail "cozy-runtime's rc-${v%%-*} holds no build"
  w=${w#cozy_runtime-}; FLOOR=">=${w%.*.dev*}.0.dev0"
fi
CODE=$OUT/rc-floor
cp -r "$(dirname "$0")/fixtures/rc-floor" "$CODE" || fail "no fixtures/rc-floor"
sed -i "s/\"cozy-runtime[^\"]*\"/\"cozy-runtime$FLOOR\"/" "$CODE/pyproject.toml"
grep -qF "\"cozy-runtime$FLOOR\"" "$CODE/pyproject.toml" || fail "the fixture's floor wasn't set to $FLOOR"
say "cozy $v, a dev home; the code needs cozy-runtime$FLOOR"
cfg=$HOME/.cozy/config.toml
grep -qxE 'dev *= *true' "$cfg" 2> /dev/null ||
  { mkdir -p "${cfg%/*}" && { echo 'dev = true'; [ ! -f "$cfg" ] || grep -v '^dev *=' "$cfg"; } > "$cfg.new" && mv "$cfg.new" "$cfg"; } || fail "dev = true not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
# What this runner's Calcifer ordered is this session's: the run rents it.
trap 'code=$?; claim; (exit "$code"); finish' EXIT
quote 1500
GUARD_ALSO=own guard
runs() { (cd "$RUN_PROJECT" && cozy --json run ls --all --on "$NAME" --limit 0) > "$OUT/$1.json" 2>&1; }
refused() { grep -qs -- "$1" "$OUT/$2.txt" "$OUT/$2.json"; }

# A project's run rents nothing itself: the rental is ordered, and the code sent to it once its Hub names its host, while it boots. A
# project's run sent by the name it had before stays on that name (43849240267: "no machine named rent-…"; the rename moves the
# machine store's runs, not a project's).
order
for _ in $(seq 900); do
  first=$(cozy machine show "$RID" --json 2> /dev/null | jq -r 'select(.placeholder == null and .state != "ended") | .name // empty'); [ -z "$first" ] || break
  sleep 1
done
[ -n "$first" ] || fail "boot: $RID was named no host in 15 min"
say "boot: the code sent to $first ($RID, $(state "$RID")) as it boots"
step boot run "$CODE/runtime" --on "$first" text=boot; boot=$?
ready
say "boot: on $NAME ($RID), image $(jq -r '.rental.base_worker_image_digest // "?"' "$OUT/show.json")"
runs boot-runs
jq -r '.runs | sort_by(.created_at_ms)[] | "  run \(.number) \(.kind) \(.status) accepted \(.accepted_at_ms // "-") settled \(.settled_at_ms // "-") \(.target | .[0:80]) \(.reason.code // "")"' "$OUT/boot-runs.json" | tee -a "$R"
if refused runtime_floor_unmet boot || refused runtime_floor_unmet boot-runs; then
  check "boot: LAB2-47826 again: the code went to $NAME on its image's Runtime and was refused runtime_floor_unmet"
elif [ "$boot" != 0 ]; then
  check "boot: the run failed: $(tail -3 "$OUT/boot.txt")"
else
  ran=$( (cd "$RUN_PROJECT" && cozy --json run watch "$(runno boot)" --once --full) | jq -r '.result.runtime // empty')
  # The first work on it was accepted only once an update of it had settled: it moved before it took work.
  first=$(jq -r '[.runs[] | select(.kind != "update") | .accepted_at_ms // empty] | min // empty' "$OUT/boot-runs.json")
  moved=$(jq -r '[.runs[] | select(.kind == "update" and .status == "completed") | .settled_at_ms] | min // empty' "$OUT/boot-runs.json")
  if [[ $ran != *.dev* ]]; then check "boot: the code ran on Runtime ${ran:-?}, not the RC's"
  elif [ -n "$moved" ] && [ -n "$first" ] && [ "$first" -lt "$moved" ]; then check "boot: work reached $NAME at $first, before its update settled at $moved"
  else say "boot PASS: the code waited for $NAME's move to the RC${moved:+ (settled $moved, first work accepted $first)} and ran on Runtime $ran"
  fi
fi

step hold machine update "$NAME" --runtime "$RUNTIME" --tensorfs "$TENSORFS" -y || fail "hold: the update by hand: $(tail -3 "$OUT/hold.txt")"
at=$(cozy machine show "$NAME" --json | jq -r '.status.runtime // empty')
[ "$at" = "$RUNTIME" ] || fail "hold: $NAME runs Runtime ${at:-?}, not $RUNTIME"
step held run "$CODE/runtime" --on "$NAME" --json text=held; held=$?
runs held-runs
if [ "$held" = 0 ]; then check "held: the code ran on $NAME, held on Runtime $RUNTIME: $(tail -3 "$OUT/held.txt")"
elif refused runtime_held held; then say "held PASS: refused runtime_held before any install, naming the update: $(grep -oE 'cozy machine update [^"]*' "$OUT/held.txt" | head -1)"
elif refused runtime_floor_unmet held || refused runtime_floor_unmet held-runs; then check "held: the install went to $NAME and was refused runtime_floor_unmet, not runtime_held before it"
else check "held: refused otherwise: $(tail -c 400 "$OUT/held.txt")"
fi
[ "$RC" != 0 ] || say "PASS: the rental moved to the RC before its first work; held by hand it refused the code by name"
exit "$RC"
