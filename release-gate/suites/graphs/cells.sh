# Suite `graphs` (gate.sh --suite graphs; tracker design cuda-graph-correctness.md; cr-287, cr-300, cr-283): CUDA graphs
# held to their calls as written, bit for bit, on a dev machine: `dev = true` in the pod's config.toml (over SSH), which
# TensorD sends in each executor's Start, so the Runtime audits every 16th replay of a region. Repeats run a package
# copy with memoization out and the differential hook in (varena/differential/lib.sh): each computes, its outputs digested.
#   graphs-ph3-t2i      P-H3-Image text-to-image: its DiT records (in the call or at idle)
#   graphs-ph3-edit-*   then an edit three times, an idle period after the first: the edit's key moved, so its DiT records
#                       at idle on its own call's inputs (cr-287: invented zero indices raced in `index_copy_`); edits 2
#                       and 3 replay it, byte-identical to edit 1 as written
#   graphs-anima-*      Anima three times and once with graphs off: its text encoder (2-D token ids, held out of idle
#                       recording until cr-300's hold went) replays, byte-identical to graphs off; again at 6 GiB
#   graphs-klein-283    Klein upsampled on cr-283's prompt and seed (gate 44006460875 run 17, rtx-4090): a decode graph
#                       that differs names its cause in the record (analyze.py fails the cell either way)
#   graphs-audit        a replay was audited against its call as written, and no P-H3-Image or Anima record differed
. "$HERE/suites/varena/differential/lib.sh"
mkdir -p "$ARMS"
gcfg="mkdir -p ~/.cozy && touch ~/.cozy/config.toml && { echo 'dev = true'; grep -v '^dev *=' ~/.cozy/config.toml; } > ~/.cozy/config.toml.new && mv ~/.cozy/config.toml.new ~/.cozy/config.toml && head -1 ~/.cozy/config.toml"
said=$(timeout 120 "$CLI" machine ssh "$M" -- "$gcfg" < /dev/null 2>&1 | tail -1)
[ "$said" = "dev = true" ] || expect graphs-dev FAIL "the pod's config.toml says no dev = true: $said"
# gcap <off|NGiB>: the machine restarts (its service reads config.toml again) with that memory limit.
gcap() {
  "$CLI" machine update "$M" --gpu-memory-limit "$1" -y > "graphs-cap-$1.log" 2>&1
  local i want=0 limit=
  [ "$1" = off ] || want=$(( ${1%GiB} << 30 ))
  for i in $(seq 12); do
    limit=$("$CLI" --json machine show "$M" 2>/dev/null | jq -r '[.status.gpus[]? | .memory_limit_bytes // 0 | tostring] | unique | join(",")')
    [ "$limit" = "$want" ] && return 0
    sleep 5
  done
  expect "graphs-cap-$1" FAIL "machine show says ${limit:-nothing} bytes, not $want"
}
gcap off
# gfact <cell> <jq over the record>: the number the record says.
gfact() { jq "$2" "$1.json" 2>/dev/null || echo 0; }
gphoto=$OUT/graphs-photo.jpg
[ -f "$gphoto" ] || nice -n 19 ffmpeg -v error -y -i "$REF" -vf "scale=w='trunc(sqrt(1048576*iw/ih)/32)*32':h=-2" -q:v 3 "$gphoto"
if diff_copy p-h3-image base ""; then
  diff_run graphs-ph3-t2i base p-h3-image generate prompt_upsampling:=false seed:=$((R + 601)) \
    instructions="A lighthouse keeper's kitchen at dawn, copper pots, steam, $OBJECT on the table (take $R)"
  gedit=(references:="[{\"name\":\"photo\",\"image\":\"$gphoto\"}]" prompt_upsampling:=false seed:=$((R + 602))
    instructions="<photo> at night under neon signs, everything else kept (take $R)")
  diff_run graphs-ph3-edit-1 base p-h3-image generate "${gedit[@]}"
  sleep 30  # an idle period: the edit's DiT records
  diff_run graphs-ph3-edit-2 base p-h3-image generate "${gedit[@]}"
  diff_run graphs-ph3-edit-3 base p-h3-image generate "${gedit[@]}"
  diff_expect graphs-ph3-repeat graphs-ph3-edit-1 graphs-ph3-edit-2
  diff_expect graphs-ph3-repeat-3 graphs-ph3-edit-1 graphs-ph3-edit-3
  tr='.measurements.residency.graphs["model.transformer"]'
  idle=$(gfact graphs-ph3-edit-2 "[$tr.graph.measured[]? | select(.replay_ms == null)] | length")
  replays=$(gfact graphs-ph3-edit-2 "$tr.replays // 0")
  eager=$(gfact graphs-ph3-edit-2 "$tr.graph.eager // [] | join(\"; \")")
  [ "$replays" -gt 0 ] && [ "$idle" -gt 0 ] && [ "$eager" = '""' ] &&
    expect graphs-ph3-idle PASS "the edit's DiT recorded at idle ($idle) and replayed $replays steps" ||
    expect graphs-ph3-idle FAIL "the edit's DiT: idle recordings $idle, replays $replays, as written for $eager"
else
  expect graphs-ph3 FAIL "no copy of p-h3-image with the hook in it"
fi
if diff_copy anima base "" && diff_copy anima eager "" eager; then
  gsays="masterpiece, best quality, anime illustration of $CHAR"
  ganima=(prompt="$gsays" seed:=$((R + 611)))
  for i in 1 2 3; do diff_run graphs-anima-$i base anima generate "${ganima[@]}"; [ "$i" != 1 ] || sleep 20; done
  diff_run graphs-anima-eager eager anima generate "${ganima[@]}"
  diff_expect graphs-anima-eager graphs-anima-eager graphs-anima-3
  diff_expect graphs-anima-repeat graphs-anima-1 graphs-anima-3
  te='[.measurements.residency.graphs | to_entries[] | select(.key | test("text_encoder")) | .value.replays // 0] | add // 0'
  replays=$(gfact graphs-anima-3 "$te")
  [ "$replays" -gt 0 ] && expect graphs-anima-replay PASS "its text encoder replayed $replays calls" ||
    expect graphs-anima-replay FAIL "its text encoder replayed nothing"
  gcap 6GiB
  for i in 1 2 3; do diff_run graphs-anima-6g-$i base anima generate prompt="$gsays" seed:=$((R + 612)); sleep 20; done
  gcap off
  diff_expect graphs-anima-6g graphs-anima-6g-1 graphs-anima-6g-3
  log "graphs-anima-6g: text encoder idle recordings $(gfact graphs-anima-6g-3 '[.measurements.residency.graphs | to_entries[] | select(.key | test("text_encoder")) | .value.graph.measured // [] | length] | add // 0'), replays $(gfact graphs-anima-6g-3 "$te")"
else
  expect graphs-anima FAIL "no copy of anima with the hook in it"
fi
cell graphs-klein-283 "$(pkg flux.3-klein generate_turbo)" prompt_upsampling:=true seed:=241107249 \
  prompt="a vintage tin robot playing a saxophone on a small stage in a smoky jazz club"
log "graphs-klein-283: decode $(jq -c '[.measurements.residency.graphs // {} | to_entries[] | select(.value.decode) | {lm: .key, captures: .value.decode.captures, refused: .value.decode.refused}]' graphs-klein-283.json 2>/dev/null)"
audited=0 mismatched=()
for f in graphs-ph3-*.json graphs-anima-*.json; do
  [ -f "$f" ] || continue
  audited=$(( audited + $(jq '[.. | objects | .calls?.audited? // 0] | add // 0' "$f" 2>/dev/null || echo 0) ))
  m=$(jq -r '[.. | objects | .safeguards?.replay_mismatch? // empty | .[]] | join(" | ")' "$f" 2>/dev/null)
  [ -z "$m" ] || mismatched+=("${f%.json}: $m")
done
for m in "${mismatched[@]}"; do log "graphs mismatch: $m"; done
[ "$audited" -gt 0 ] && [ "${#mismatched[@]}" = 0 ] && expect graphs-audit PASS "$audited replays audited, none differed" ||
  expect graphs-audit FAIL "$audited replays audited; ${#mismatched[@]} records with a replay_mismatch: ${mismatched[*]:0:2}"
