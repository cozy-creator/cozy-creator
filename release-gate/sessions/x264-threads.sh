# x264-threads (xs-220, hack audit runtime #8): a video save's x264 threads on one CPU rental (sku=cpu) of 16 or more vCPUs, the
# image's Runtime (at most 8 threads, X264_THREAD_CEILING) against the change's (every CPU this process may use). Each arm saves
# the same 1344x768 clip through `cozy run` (fixtures/x264/save.py) and says its threads, its CPUs and the save's seconds; the
# change's must use more than 8 threads and save faster. Without `rent`, $0: the quote, and the checks before it.
# params:
#   runtime  the cozy-runtime dev-build run id of the change's Runtime (required)
#   frames   the clip's frames (default 720: 30 s at 24 fps, 2.2 GB of frames, under the 4 GiB a save may snapshot)
#   min_cpus the fewest CPUs the rental must give for the arms to differ (default 16)
#   seconds  what the cap must buy (default 1200)
#   local    true: both arms on the runner's own machine, no rental and no comparison: the fixture's path checked for $0
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/x264/save.py
SRC=$(param runtime '') FRAMES=$(param frames 720) MIN=$(param min_cpus 16) LOCAL=$(param local false)
[ -f "$FIX" ] || fail "no fixtures/x264/save.py"
[[ $SRC =~ ^[0-9]+$ ]] || fail "runtime: a cozy-runtime dev-build run id"
W=$(wheel_from "$OUT/wheel" "$SRC") && [ -n "$W" ] || fail "no manylinux wheel in cozy-runtime run $SRC"
say "runtime: ${W##*/}"
if [ "$LOCAL" = true ]; then
  NAME=local; say "local: the runner's own machine, the fixture's path only (no rental, no comparison)"
else
  quote "$(param seconds 1200)"
  # Only the listed offer of $MIN vCPUs: a quote for another (a cheaper offer of fewer, 11-19: 4 vCPU at $0.25/h for a cpu listed
  # as 16 at $0.834/h) is refused before anything is rented.
  listed=$(cozy --json machine rent < /dev/null 2> /dev/null | jq -r --argjson m "$MIN" '.[]? | select(.name == "cpu") | .widths[] | select((.vcpu_count // 0) >= $m) | .price_usd_micros_per_hour' | head -1)
  quoted=$(jq -r '.quote.usd_micros_per_hour // empty' "$OUT/quote.json")
  [ -n "$listed" ] && [ "$quoted" = "$listed" ] || fail "the quote is ${quoted:-none} micro-USD/h, not the listed $MIN-vCPU offer's ${listed:-none}: nothing rented"
  rent
fi
# arm <name>: the clip saved on $NAME twice and the second exported; its threads (from the mp4's x264 settings), CPUs and encode seconds (the run's
# start to its settling, less the call's own time making the frames) in <name>.facts, and said.
arm() {
  # Once to warm its executor and encoder (the local check's first arm took 2.92 s, its second 1.7 s, at the same 4 threads), then
  # the save measured.
  step "$1-warm" run "$FIX/save" "arm=$1-warm" "frames:=$FRAMES" --on "$NAME" --output "$OUT/$1-warm" || fail "$1: the warm save failed: $(tail -c 300 "$OUT/$1-warm.txt")"
  step "$1" run "$FIX/save" "arm=$1" "frames:=$FRAMES" --on "$NAME" --output "$OUT/$1" || fail "$1: the save failed: $(tail -c 300 "$OUT/$1.txt")"
  local n mp4 threads cpus secs
  n=$(runno "$1")
  cozy run watch "$n" --once --json --full > "$OUT/$1.json" 2>&1
  jq -e '.status == "completed"' "$OUT/$1.json" > /dev/null || fail "$1: run $n is $(jq -r .status "$OUT/$1.json")"
  mp4=$(find "$OUT/$1" -name '*.mp4' | head -1)
  [ -n "$mp4" ] || fail "$1: run $n exported no mp4 to $OUT/$1"
  threads=$(grep -aoE ' threads=[0-9]+' "$mp4" | head -1 | cut -d= -f2)
  [ -n "$threads" ] || fail "$1: $mp4 carries no x264 settings"
  cpus=$(jq -r '.result.value.cpus // .result.cpus // empty' "$OUT/$1.json")
  secs=$(jq -r '((.settled_at_ms - .started_at_ms) / 1000 - (.result.value.made_s // .result.made_s)) * 100 | round / 100' "$OUT/$1.json" 2>/dev/null)
  [ -n "$cpus" ] && [ -n "$secs" ] || fail "$1: run $n's record lacks cpus, made_s or its times: $(jq -c '{started_at_ms, settled_at_ms, result}' "$OUT/$1.json" | head -c 300)"
  printf '{"threads": %s, "cpus": %s, "save_s": %s}\n' "$threads" "$cpus" "$secs" > "$OUT/$1.facts"
  echo "$1: run $n, $threads x264 threads of $cpus CPUs, encoded in $secs s" | tee -a "$R"
}
result() { jq -r ".$2" "$OUT/$1.facts"; }
arm image
say "image Runtime: $(cozy machine show "$NAME" --json | jq -r .status.runtime)"
[ "$LOCAL" = true ] || [ "$(result image cpus)" -ge "$MIN" ] || fail "$NAME gives $(result image cpus) CPUs, under the $MIN the arms need to differ"
step update machine update "$NAME" --runtime "$W" -y || fail "the change's Runtime on $NAME"
say "change's Runtime: $(cozy machine show "$NAME" --json | jq -r .status.runtime)"
arm change
if [ "$LOCAL" = true ]; then
  [ "$(result change threads)" -le "$(result change cpus)" ] || fail "change: $(result change threads) threads of $(result change cpus) CPUs"
  say "PASS (local path): image $(result image threads) threads in $(result image save_s) s, change $(result change threads) of $(result change cpus) CPUs in $(result change save_s) s"
  exit "$RC"
fi
[ "$(result image threads)" -le 8 ] || fail "image: $(result image threads) threads, past its ceiling of 8"
[ "$(result change threads)" = "$(result change cpus)" ] || fail "change: $(result change threads) threads of $(result change cpus) CPUs"
awk -v a="$(result image save_s)" -v b="$(result change save_s)" 'BEGIN { exit !(b < a) }' ||
  fail "change: saved in $(result change save_s) s, not under the image's $(result image save_s) s"
say "PASS: $(result change threads) threads saved in $(result change save_s) s against 8 in $(result image save_s) s"
exit "$RC"
