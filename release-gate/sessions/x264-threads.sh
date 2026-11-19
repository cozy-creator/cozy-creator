# x264-threads (xs-220, hack audit runtime #8): a video save's x264 threads on one CPU rental (sku=cpu) of 16 or more vCPUs, the
# image's Runtime (at most 8 threads, X264_THREAD_CEILING) against the change's (every CPU this process may use). Each arm saves
# the same 1344x768 clip through `cozy run` (fixtures/x264/save.py) and says its threads, its CPUs and the save's seconds; the
# change's must use more than 8 threads and save faster. Without `rent`, $0: the quote, and the checks before it.
# params:
#   runtime  the cozy-runtime dev-build run id of the change's Runtime (required)
#   frames   the clip's frames (default 360: 15 s at 24 fps)
#   min_cpus the fewest CPUs the rental must give for the arms to differ (default 16)
#   seconds  what the cap must buy (default 1200)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/x264/save.py
SRC=$(param runtime '') FRAMES=$(param frames 360) MIN=$(param min_cpus 16)
[ -f "$FIX" ] || fail "no fixtures/x264/save.py"
[[ $SRC =~ ^[0-9]+$ ]] || fail "runtime: a cozy-runtime dev-build run id"
W=$(wheel_from "$OUT/wheel" "$SRC") && [ -n "$W" ] || fail "no manylinux wheel in cozy-runtime run $SRC"
say "runtime: ${W##*/}"
quote "$(param seconds 1200)"
rent
# arm <name>: the clip saved on $NAME; its record in <name>.json, its threads, CPUs and save seconds said.
arm() {
  step "$1" run "$FIX/save" "arm=$1" "frames:=$FRAMES" --on "$NAME" --output "$OUT/$1" || fail "$1: the save failed: $(tail -c 300 "$OUT/$1.txt")"
  local n
  n=$(grep -m1 -oE '^run [A-Z][A-Z0-9]*-[0-9]+' "$OUT/$1.txt" | awk '{print $2}')
  cozy run watch "$n" --once --json --full > "$OUT/$1.json" 2>&1
  jq -e '.status == "completed"' "$OUT/$1.json" > /dev/null || fail "$1: run $n is $(jq -r .status "$OUT/$1.json")"
  jq -r --arg a "$1" --arg n "$n" '.result.value // .result | "\($a): run \($n), \(.threads) x264 threads of \(.cpus) CPUs, saved in \(.save_s * 10 | round / 10) s"' "$OUT/$1.json" | tee -a "$R"
}
result() { jq -r ".result.value.$2 // .result.$2" "$OUT/$1.json"; }
arm image
say "image Runtime: $(cozy machine show "$NAME" --json | jq -r .status.runtime)"
[ "$(result image cpus)" -ge "$MIN" ] || fail "$NAME gives $(result image cpus) CPUs, under the $MIN the arms need to differ"
step update machine update "$NAME" --runtime "$W" -y || fail "the change's Runtime on $NAME"
say "change's Runtime: $(cozy machine show "$NAME" --json | jq -r .status.runtime)"
arm change
[ "$(result image threads)" -le 8 ] || fail "image: $(result image threads) threads, past its ceiling of 8"
[ "$(result change threads)" = "$(result change cpus)" ] || fail "change: $(result change threads) threads of $(result change cpus) CPUs"
awk -v a="$(result image save_s)" -v b="$(result change save_s)" 'BEGIN { exit !(b < a) }' ||
  fail "change: saved in $(result change save_s) s, not under the image's $(result image save_s) s"
say "PASS: $(result change threads) threads saved in $(result change save_s) s against 8 in $(result image save_s) s"
exit "$RC"
