# Suite `graphs` (gate.sh --suite graphs; tracker design cuda-graph-correctness.md; cr-287, cr-300, cr-283): CUDA graphs
# held to their calls as written, bit for bit, on a dev machine: `dev = true` in the pod's config.toml (over SSH), which
# TensorD sends in each executor's Start, so the Runtime audits every 16th replay of a region. Repeats run a package
# copy with memoization out, so a second request computes; equal output bytes mean a replay equals the call as written.
#   graphs-ph3-t2i      P-H3-Image text-to-image: its DiT records in the call
#   graphs-ph3-short-*  then a shorter prompt three times, an idle period after the first: its key moved, so its DiT records
#                       at idle on its own call's inputs (cr-287: invented zero indices raced in `index_copy_`); 2 and 3
#                       replay it, their images byte-identical to 1's as written
#   graphs-anima-*      Anima three times: its text encoder (2-D token ids, held out of idle recording until cr-300's hold
#                       went) replays in the third, its image byte-identical to the first's; again at 6 GiB
#   graphs-sdxl-*       SDXL at one size, then another twice with an idle period between: the new size's UNet records at
#                       idle (its device reads answered by its call as written) and replays, its image byte-identical
#   graphs-klein-283    Klein upsampled on cr-283's prompt and seed (gate 44006460875 run 17, rtx-4090): a decode graph
#                       that differs names its cause in the record (analyze.py fails the cell either way)
#   graphs-h3-upscale-* Regenerate-2K of a 5 s plate at 1080p, three prompts: the latent upscaler captures its graph (its
#                       scale embedding was a pageable host copy, refused: gate 44167820873)
#   graphs-audit        replays were audited against their calls as written, and no P-H3-Image or Anima record differed
G=$OUT/graphs
mkdir -p "$G"
pod_dev "$CLI" "$M" > /dev/null && expect graphs-dev PASS "the pod's config.toml says dev = true" ||
  expect graphs-dev FAIL "the pod's config.toml says no dev = true"
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
# gcopy <package>: the checkout's package with its memoization out, at ./graphs/<package>.
gcopy() {
  rm -rf "${G:?}/$1"; mkdir -p "$G/$1"
  tar -C "$PKGS/$1" --exclude=.venv --exclude=__pycache__ -cf - . | tar -C "$G/$1" -xf - &&
    { grep -rlE --include='*.py' 'memoize=' "$G/$1" || true; } |
    xargs -r sed -i -E -e '/^ *memoize=(True|Memoize\(requires=\["seed"\]\)),$/d' -e 's/, *memoize=True//; s/memoize=True, *//'
}
# grun <cell> <package> <function> <args...>: a run of the copy (its record graphs/<cell>.json via run_cell).
grun() { local name=$1 package=$2 function=$3; shift 3; run_cell "$OUT" "$name" "./graphs/$package/$function" --on "$M" "$@"
  log "$name run $(cat "$name.number" 2>/dev/null) $(run_get "$OUT" "$name" .status)"; }
# gsame <check> <a> <b>: both completed and their outputs are the same bytes.
gsame() {
  local a b
  a=$(jq -r 'select(.status == "completed") | [.output[]?.sha256] | join(",")' "$2.json" 2>/dev/null)
  b=$(jq -r 'select(.status == "completed") | [.output[]?.sha256] | join(",")' "$3.json" 2>/dev/null)
  [ -n "$a" ] && [ "$a" = "$b" ] && expect "$1" PASS "$2 and $3: the same bytes ($a)" ||
    expect "$1" FAIL "$2 ${a:-did not complete} against $3 ${b:-did not complete}"
}
gfact() { jq "$2" "$1.json" 2>/dev/null || echo 0; }
# gwant <tag>: whether --only (if any) asks for this part (graphs-ph3, graphs-anima, graphs-sdxl).
gwant() { [ -z "$ONLY" ] || [[ $ONLY == *",$1,"* ]]; }
# gregion <regex> <field jq>: over every region of a record whose name matches, the field's values in a list.
gregion() { echo "[.. | objects | .graphs? // empty | objects | to_entries[] | select(.key | test(\"$1\")) | .value | $2]"; }
if ! gwant graphs-ph3; then :
elif gcopy p-h3-image; then
  # A text-to-image records its DiT in the call; another prompt's key then moved, so a short step (under the
  # Runtime's 1 s idle bound: a 4090's edit DiT step is past it) records at idle on its own call's inputs.
  grun graphs-ph3-t2i p-h3-image generate prompt_upsampling:=false seed:=$((R + 601)) \
    instructions="A lighthouse keeper's kitchen at dawn, copper pots, steam, $OBJECT on the table (take $R)"
  gshort=(prompt_upsampling:=false seed:=$((R + 602)) instructions="$OBJECT, studio photo")
  grun graphs-ph3-short-1 p-h3-image generate "${gshort[@]}"
  sleep 30  # an idle period: the short prompt's DiT records
  for i in 2 3; do grun graphs-ph3-short-$i p-h3-image generate "${gshort[@]}"; done
  gsame graphs-ph3-repeat graphs-ph3-short-1 graphs-ph3-short-2
  gsame graphs-ph3-repeat-3 graphs-ph3-short-1 graphs-ph3-short-3
  idle=$(gfact graphs-ph3-short-2 "$(gregion transformer '.graph.measured[]? | select(.replay_ms == null)') | length")
  replays=$(gfact graphs-ph3-short-2 "$(gregion transformer '.replays // 0') | add // 0")
  eager=$(gfact graphs-ph3-short-2 "$(gregion transformer '.graph.eager // [] | .[]') | join(\"; \")")
  [ "$replays" -gt 0 ] && [ "$idle" -gt 0 ] && [ "$eager" = '""' ] &&
    expect graphs-ph3-idle PASS "the short prompt's DiT recorded at idle ($idle) and replayed $replays steps" ||
    expect graphs-ph3-idle FAIL "the short prompt's DiT: idle recordings $idle, replays $replays, as written for $eager"
else
  expect graphs-ph3 FAIL "no copy of p-h3-image"
fi
if gwant graphs-sdxl && gcopy sdxl; then
  # SDXL at one size, then another (its key moved: its UNet records at idle, a call's device reads answered by its run as
  # written, once declined at idle), then that size again, replaying from its first step: its image byte-identical.
  grun graphs-sdxl-a sdxl generate prompt="$OBJECT on a wooden table, photo" seed:=$((R + 621))
  gsize=(prompt="$SCENE at dusk, wide shot, photo" seed:=$((R + 622)) aspect_ratio=16:9)
  grun graphs-sdxl-b-1 sdxl generate "${gsize[@]}"
  sleep 15  # an idle period: the new size's UNet records
  grun graphs-sdxl-b-2 sdxl generate "${gsize[@]}"
  gsame graphs-sdxl-idle-bytes graphs-sdxl-b-1 graphs-sdxl-b-2
  idle=$(gfact graphs-sdxl-b-2 "$(gregion unet '.graph.measured[]? | select(.replay_ms == null)') | length")
  replays=$(gfact graphs-sdxl-b-2 "$(gregion unet '.replays // 0') | add // 0")
  eager=$(gfact graphs-sdxl-b-2 "$(gregion unet '.graph.eager // [] | .[]') | join(\"; \")")
  [ "$replays" -gt 0 ] && [ "$idle" -gt 0 ] && [ "$eager" = '""' ] &&
    expect graphs-sdxl-idle PASS "the new size's UNet recorded at idle ($idle) and replayed $replays steps" ||
    expect graphs-sdxl-idle FAIL "the new size's UNet: idle recordings $idle, replays $replays, as written for $eager"
fi
if ! gwant graphs-anima; then :
elif gcopy anima; then
  gsays="masterpiece, best quality, anime illustration of $CHAR"
  for i in 1 2 3; do grun graphs-anima-$i anima generate prompt="$gsays" seed:=$((R + 611)); [ "$i" != 1 ] || sleep 20; done
  gsame graphs-anima-repeat graphs-anima-1 graphs-anima-3
  replays=$(gfact graphs-anima-3 "$(gregion text_encoder '.replays // 0') | add // 0")
  [ "$replays" -gt 0 ] && expect graphs-anima-replay PASS "its text encoder replayed $replays calls" ||
    expect graphs-anima-replay FAIL "its text encoder replayed nothing"
  gcap 6GiB
  for i in 1 2 3; do grun graphs-anima-6g-$i anima generate prompt="$gsays" seed:=$((R + 612)); sleep 20; done
  gcap off
  gsame graphs-anima-6g graphs-anima-6g-1 graphs-anima-6g-3
  log "graphs-anima-6g: text encoder idle recordings $(gfact graphs-anima-6g-3 "$(gregion text_encoder '.graph.measured[]? | select(.replay_ms == null)') | length"), replays $(gfact graphs-anima-6g-3 "$(gregion text_encoder '.replays // 0') | add // 0")"
else
  expect graphs-anima FAIL "no copy of anima"
fi
[ -n "$ONLY" ] || cell graphs-klein-283 "$(pkg flux.3-klein generate_turbo)" prompt_upsampling:=true seed:=241107249 \
  prompt="a vintage tin robot playing a saxophone on a small stage in a smoky jazz club"
log "graphs-klein-283: decode $(jq -c '[.measurements.residency.graphs // {} | to_entries[] | select(.value.decode) | {lm: .key, captures: .value.decode.captures, refused: .value.decode.refused}]' graphs-klein-283.json 2>/dev/null)"
if gwant graphs-h3; then
  gplate=$(box graphs_plate "$(plate 5)" "A test pattern with a tone")
  for i in 1 2 3; do
    cell graphs-h3-upscale-$i "$(pkg minimax-h3 upscale)" video="$gplate" to=1080p prompt="$SCENE, take $i, slow push-in, ambient sound"
  done
  up="$(gregion 'upscaler$' '.captures // 0') | add // 0"
  refused=$(cat graphs-h3-upscale-[123].json 2>/dev/null | jq -s -r '[.[] | .. | objects | .capture_refused? // empty | .[] | select(test("upscaler"))] | join(" | ")')
  captures=$(( $(gfact graphs-h3-upscale-2 "$up") + $(gfact graphs-h3-upscale-3 "$up") ))
  [ "$captures" -gt 0 ] && [ -z "$refused" ] && expect graphs-h3-upscaler PASS "the latent upscaler captured $captures graphs" ||
    expect graphs-h3-upscaler FAIL "the latent upscaler captured $captures; refused: ${refused:-none}"
fi
audited=0 mismatched=()
for f in graphs-*.json; do
  [ -f "$f" ] || continue
  audited=$(( audited + $(jq '[.. | objects | .calls?.audited? // 0] | add // 0' "$f" 2>/dev/null || echo 0) ))
  [[ $f == graphs-klein-283.json ]] && continue
  m=$(jq -r '[.. | objects | .safeguards?.replay_mismatch? // empty | .[]] | join(" | ")' "$f" 2>/dev/null)
  [ -z "$m" ] || mismatched+=("${f%.json}: $m")
done
for m in "${mismatched[@]}"; do log "graphs mismatch: $m"; done
[ "$audited" -gt 0 ] && [ "${#mismatched[@]}" = 0 ] && expect graphs-audit PASS "$audited replays audited, none differed" ||
  expect graphs-audit FAIL "$audited replays audited; ${#mismatched[@]} records with a replay_mismatch: ${mismatched[*]:0:2}"
