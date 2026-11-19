# Suite `graphs` (gate.sh --suite graphs; tracker design cuda-graph-correctness.md; cr-287, cr-300, cr-283, cr-333): CUDA graphs
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
#   graphs-lm-again-*   the describer (Qwen3.5-9B) asked 8 prompts at once, then again in the same executor with a longer
#                       system prompt: its decode slots regrow, so the second call records and checks its graphs anew
#                       (cr-333: MiniMax-H3-Image's expander failed its second call `device_fault`; pod-session 44244508049)
#   graphs-lm-checks    no decode graph's check, in any record here, saved more than 64 MiB beyond its decode state (a dev
#                       machine's `decode.partial_restore`). Runtime 0.48.0.dev2326 to dev2333 saved every key buffer whole:
#                       153 MiB for Klein's one row (gate 44205178562), 10 GiB for 16 rows of Qwen3.8-27B (cr-333)
#   graphs-h3-upscale-* Regenerate-2K of a 5 s plate at 1080p, three prompts: the latent upscaler captures its graph (its
#                       scale embedding was a pageable host copy, refused: gate 44167820873)
#   graphs-audit        replays were audited against their calls as written, and no P-H3-Image or Anima record differed
#   graphs-operands     var-034, the coordinator's rule (11-18): "a replay never writes to or reads memory its graph doesn't
#                       hold … a replay refuses in one line when that is gone. Never a GPU fault from dropping a reference."
#                       operands.py under the machine's own TensorFS, one process a case: a graph whose output was dropped
#                       replays, one whose operands were freed refuses in one line. Last: without the guard that replay
#                       faults the GPU (Xid 31; pod-session 44430279106)
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
cell graphs-klein-283 "$(pkg flux.3-klein generate_turbo)" prompt_upsampling:=true seed:=241107249 \
  prompt="a vintage tin robot playing a saxophone on a small stage in a smoky jazz club"
log "graphs-klein-283: decode $(jq -c '[.measurements.residency.graphs // {} | to_entries[] | select(.value.decode) | {lm: .key, captures: .value.decode.captures, replays: .value.decode.replays, refused: .value.decode.refused, partial: .value.decode.partial_restore}]' graphs-klein-283.json 2>/dev/null)"
if gwant graphs-lm; then
  # lrules <n>: n short rules, about 8 tokens each. The second call's 75 more are past one window of 256 positions, the
  # step its slots' capacity grows by, so it drops the first call's slots and records its graphs again.
  lrules() { local i out=; for i in $(seq "$1"); do out+="Rule $i: keep to what the request names. "; done; echo "$out"; }
  lasks=$(jq -cn --arg a "$OBJECT" --arg b "$SCENE" --arg c "$CHAR" '[range(8) | {text: "Request \(. + 1): \([$a, $b, $c][. % 3]), view \(. + 1)"}]')
  lask() {
    cell "$1" "$(pkg qwen3 ask)" --model model=qwen/qwen3.5-9b@0.2.0 prompts:="$lasks" max_tokens:=256 seed:=$((R + 631)) \
      system="Expand each request into one caption of about 60 words (take $R). $(lrules "$2")"
  }
  lask graphs-lm-again-1 75
  lask graphs-lm-again-2 150
  ldecode='[.measurements.residency.graphs // {} | to_entries[] | select(.value.decode) | .value.decode] | first // {}'
  lnew=$(( $(gfact graphs-lm-again-2 "$ldecode | .captures // 0") - $(gfact graphs-lm-again-1 "$ldecode | .captures // 0") ))
  lrefused=$(jq -r "$ldecode | .refused // [] | join(\" | \")" graphs-lm-again-2.json 2>/dev/null)
  # A call that started its own executor is a first call: its record has an `executor start` stage.
  lfresh=$(gfact graphs-lm-again-2 '[.report.measured[]?.stages[]? | select(.name == "executor start")] | length')
  [ "$lfresh" = 0 ] && [ "$lnew" -gt 0 ] && [ -z "$lrefused" ] &&
    expect graphs-lm-again PASS "the second call, in the first's executor, recorded $lnew decode graphs and kept them" ||
    expect graphs-lm-again FAIL "the second call: new executor $lfresh, decode graphs recorded $lnew, refused: ${lrefused:-none}"
fi
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
# What each decode graph's check saved beyond its decode state, as a dev machine's record says it ("N more tensors put back
# (X KiB)"): the slots a step writes, never a key buffer (cr-333).
lsaved=$(cat graphs-*.json 2>/dev/null | jq -s -r '[.[] | .. | objects | .partial_restore? // empty | .[] | strings
  | capture("put back \\((?<n>[0-9]+) (?<u>[KMG])iB\\)") | (.n | tonumber) * {K: 1024, M: 1048576, G: 1073741824}[.u]]
  | "\(length) \(max // 0)"')
read -r lchecks lmost <<< "${lsaved:-0 0}"
# 64 MiB: a step's slots (4 positions a row of each key buffer) were 1 MiB at 8 rows of Qwen3.5-9B (gate 44282226329); whole
# buffers were 153 MiB at Klein's one row. A --only that asks for no LM cell may run no decode check: nothing to judge.
if [ "${lchecks:-0}" = 0 ] && ! gwant graphs-lm; then
  expect graphs-lm-checks SKIP "no decode check ran: --only asked for no graphs-lm cell"
elif [ "${lchecks:-0}" -gt 0 ] && [ "${lmost:-0}" -le $((64 << 20)) ]; then
  expect graphs-lm-checks PASS "$lchecks decode checks, the largest saved $((lmost >> 10)) KiB beyond its decode state"
else
  expect graphs-lm-checks FAIL "$lchecks decode checks said what they saved; the largest $((${lmost:-0} >> 20)) MiB beyond its decode state (64 MiB at most)"
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
# Every 16th replay of a region is audited: only P-H3-Image's, SDXL's and Anima's repeats replay that often (a lone
# graphs-klein-283 replays its DiT 13 times). A --only that asks for none of them audits nothing: nothing to judge.
if [ "$audited" = 0 ] && [ "${#mismatched[@]}" = 0 ] && ! gwant graphs-ph3 && ! gwant graphs-sdxl && ! gwant graphs-anima; then
  expect graphs-audit SKIP "no replay audited: --only asked for no graphs-ph3, graphs-sdxl or graphs-anima cell"
elif [ "$audited" -gt 0 ] && [ "${#mismatched[@]}" = 0 ]; then
  expect graphs-audit PASS "$audited replays audited, none differed"
else
  expect graphs-audit FAIL "$audited replays audited; ${#mismatched[@]} records with a replay_mismatch: ${mismatched[*]:0:2}"
fi
if gwant graphs-operands; then
  # The machine's executor environment: an earlier cell's, else the SDXL copy's install makes one.
  gop_py() { timeout 60 "$CLI" machine ssh "$M" -- "ls -t /var/lib/cozy/tensord/generations/*/env/bin/python 2> /dev/null | head -1" < /dev/null 2> /dev/null | tail -n 1; }
  gopy=$(gop_py)
  if [[ $gopy != /var/lib/cozy/* ]] && gcopy sdxl; then
    "$CLI" package install ./graphs/sdxl --on "$M" > graphs-operands-install.log 2>&1; gopy=$(gop_py)
  fi
  # gop_said <case> <jq>: that case's report, read by the filter.
  gop_said() { grep -E '^[{]' "graphs-operands-$1.txt" 2> /dev/null | tail -n 1 | jq -r "$2" 2> /dev/null; }
  if [[ $gopy != /var/lib/cozy/* ]]; then
    expect graphs-operands FAIL "no executor environment on $M: ${gopy:-none}"
  elif ! timeout 60 "$CLI" machine ssh "$M" -- "cat > /tmp/graphs-operands.py" < "$HERE/suites/graphs/operands.py" > /dev/null 2>&1; then
    expect graphs-operands FAIL "operands.py did not reach $M"
  else
    for c in outputs operands; do  # one process a case: a fault ends its context
      timeout 300 "$CLI" machine ssh "$M" -- "$gopy /tmp/graphs-operands.py 0 $c" < /dev/null > "graphs-operands-$c.txt" 2> "graphs-operands-$c.err"
      log "graphs-operands $c: $(gop_said "$c" tojson | head -c 500)"
    done
    gout=$(gop_said outputs 'select(.replayed == true) | .tensorfs') gops=$(gop_said operands '.refused // empty')
    [ -n "$gout" ] && [ -n "$gops" ] && expect graphs-operands PASS "TensorFS $gout: a dropped output replays; freed operands refuse: $gops" ||
      expect graphs-operands FAIL "outputs: $(gop_said outputs tojson | grep . || echo 'no report'); operands: $(gop_said operands tojson | grep . || echo 'no report')"
  fi
fi
