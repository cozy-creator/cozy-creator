# Suite `inputs` (cr-279): each input that grows a component's working memory, at the most its field takes, per package with
# a text path, on a small card and under a cap. Paul 11-12: memory errors like run 46479's "cannot happen in practice"; what
# can't fit is refused at once with its numbers. A cell passes when it completes; any failure fails it, a capacity one
# (device_out_of_memory, memory_exhausted, does_not_fit) above all.
# - ph3-46479: P-H3-Image's edit of one picture, upsampling on, as run 46479. On Runtime 0.45.0 its upsampler's decode-step
#   capture ran short of its pool (window 2048 of the picture's ~1,100 prompt tokens) and the run failed in 0.9 s. With a
#   runtime_wheel, it first runs on the Runtime the pod came with (ph3-46479-released, recorded, not judged).
# - <cell>-max: the longest prompt (65,536 characters, the describer's field limit, where a field sets none), the most
#   pictures the function takes at the largest size it reads, and its prompt LM on; -noup the same with the LM off.
# - describer-max: 16 pictures, the longest prompt and the longest answer (max_tokens 16384) the describer's fields allow.
# - <cell>-cap8: the LM-heavy cells again with the GPU capped at 8 GiB (`machine update --gpu-memory-limit`).
LONG=65536
# words <chars> <mentions>: a scene described at length, <chars> long, every sentence naming the pictures.
words() {
  local n=$1 m=${2:-} i=0 out='' when=("at dawn" "in light rain" "under a full moon" "in late autumn" "at noon" "in deep winter")
  while [ ${#out} -lt "$n" ]; do
    i=$((i + 1))
    out+="Detail $i: $SCENE ${when[$((i % 6))]}, $OBJECT in the foreground${m:+ as in $m}, lit from the left, every surface sharp. "
  done
  printf '%s' "${out:0:$n}"
}
# pictures <prefix> <count> <megapixels>: distinct JPEGs of the gate's reference at that size, as $PICS (references JSON) and
# $MENTIONS (each `<name>`).
pictures() {
  local p=$1 n=$2 mp=$3 i rows=()
  for ((i = 1; i <= n; i++)); do
    [ -f "$OUT/$p$i.jpg" ] || nice -n 19 ffmpeg -v error -y -i "$REF" -vf \
      "hue=h=$((i * 37)),scale=w='trunc(sqrt($mp*1048576*iw/ih)/32)*32':h=-2" -q:v 3 "$OUT/$p$i.jpg"
    rows+=("{\"name\":\"$p$i\",\"image\":\"$OUT/$p$i.jpg\"}")
  done
  PICS="[$(IFS=,; echo "${rows[*]}")]" MENTIONS=$(for ((i = 1; i <= n; i++)); do printf '<%s%d> ' "$p" "$i"; done)
}
# fits <cell>: completed, or why not.
fits() {
  local c=$1${TWIN:-} s
  [ -f "$c.number" ] || return 0
  s=$(run_get "$OUT" "$c" .status)
  expect "$1-fits" "$([ "$s" = completed ] && echo PASS || echo FAIL)" "run $(cat "$c.number") $s \
$(run_get "$OUT" "$c" '(.reason.code // "") + " " + (.reason.message // "")' | head -c 300)"
}
PH3_46479="<p1> make her hair light brunette pastel pink, she holds a small pistol in her right hand. Get rid of the orange blob at the bottom left"
INPUTS_RELEASED=$(awk '$1 == "runtime" {print $2}' machine-fresh.txt)
if [ -n "$RWF" ] && [ -n "$INPUTS_RELEASED" ] && [ "$INPUTS_RELEASED" != "$rt" ] && { [ -z "$ONLY" ] || [[ $ONLY == *,ph3-46479,* ]]; }; then
  pictures p 1 2
  if "$CLI" machine update "$M" --runtime "$INPUTS_RELEASED" -y > inputs-released.log 2>&1; then
    run_cell "$OUT" ph3-46479-released "$(pkg minimax-h3 image)" --on "$M" references:="$PICS" seed:=$((R + 900)) \
      instructions="$PH3_46479" > /dev/null 2>&1 || true
    log "inputs: ph3-46479 on Runtime $INPUTS_RELEASED: run $(cat ph3-46479-released.number 2>/dev/null) \
$(run_get "$OUT" ph3-46479-released '.status + " " + (.reason.code // "") + " " + (.reason.message // "")' | head -c 300)"
  else
    log "inputs: Runtime $INPUTS_RELEASED did not go on: $(tail -c 300 inputs-released.log)"
  fi
  "$CLI" machine update "$M" --runtime "$RWF" -y > inputs-candidate.log 2>&1 ||
    { expect inputs-candidate FAIL "the candidate did not go back on: $(tail -c 300 inputs-candidate.log)"; return 0; }
fi
run_inputs() {  # run_inputs <suffix> <seed offset>: every cell, named with <suffix>, its seed past the other passes
  local x=$1 o=$2
  pictures p 1 2
  cell "ph3-46479$x" "$(pkg minimax-h3 image)" references:="$PICS" seed:=$((R + 900 + o)) instructions="$PH3_46479"
  fits "ph3-46479$x"
  cell "ph3-t2i-max$x" "$(pkg minimax-h3 image)" instructions="$(words $LONG)" megapixels:=16 prompt_upsampling:=true seed:=$((R + 901 + o))
  fits "ph3-t2i-max$x"
  cell "ph3-t2i-noup$x" "$(pkg minimax-h3 image)" instructions="$(words $LONG)" megapixels:=16 prompt_upsampling:=false seed:=$((R + 902 + o))
  fits "ph3-t2i-noup$x"
  pictures e 8 8
  cell "ph3-edit-max$x" "$(pkg minimax-h3 image)" references:="$PICS" instructions="$MENTIONS$(words $LONG "$MENTIONS")" \
    megapixels:=16 prompt_upsampling:=true seed:=$((R + 903 + o))
  fits "ph3-edit-max$x"
  cell "ph3-edit-noup$x" "$(pkg minimax-h3 image)" references:="$PICS" instructions="$MENTIONS$(words $LONG "$MENTIONS")" \
    megapixels:=16 prompt_upsampling:=false seed:=$((R + 904 + o))
  fits "ph3-edit-noup$x"
  pictures k 10 4
  cell "klein-max$x" "$(pkg flux.3 generate_turbo)" references:="$PICS" prompt="$MENTIONS$(words $LONG "$MENTIONS")" \
    megapixels:=4 prompt_upsampling:=true seed:=$((R + 905 + o))
  fits "klein-max$x"
  cell "qwen2-max$x" "$(pkg qwen-image-2 generate)" references:="$PICS" prompt="$MENTIONS$(words $LONG "$MENTIONS")" \
    megapixels:=4 prompt_upsampling:=true seed:=$((R + 906 + o))
  fits "qwen2-max$x"
  cell "flux2-max$x" "$(pkg flux-2 generate_turbo)" references:="$PICS" prompt="$MENTIONS$(words $LONG "$MENTIONS")" \
    megapixels:=4 prompt_upsampling:=true seed:=$((R + 907 + o))
  fits "flux2-max$x"
  # Its edit's text rotary table ended at 4,051 rows for 22,526 tokens (se-119): a prompt it reads.
  cell "qwen-image-max$x" "$(pkg qwen-image generate)" references:="$PICS" prompt="$MENTIONS$(words 8000 "$MENTIONS")" \
    megapixels:=2 seed:=$((R + 908 + o))
  fits "qwen-image-max$x"
  pictures f 4 1
  cell "fibo-max$x" "$(pkg fibo-2 generate)" references:="$PICS" prompt="$MENTIONS$(words $LONG "$MENTIONS")" \
    megapixels:=1 prompt_upsampling:=true seed:=$((R + 909 + o))
  fits "fibo-max$x"
  cell "ideogram-max$x" "$(pkg ideogram-4 generate)" prompt="$(words $LONG)" prompt_upsampling:=true seed:=$((R + 910 + o))
  fits "ideogram-max$x"
  pictures d 16 2
  # The describer's images bind by kind, as positional files.
  cell "describer-max$x" "$(pkg qwen3.5 ask)" prompt="$(words $LONG)" max_tokens:=16384 seed:=$((R + 912 + o)) \
    $(jq -r '.[].image' <<< "$PICS")
  fits "describer-max$x"
  # H3 calls its segments with the prompt inline, at most 48 KiB (cr-281): its longest that runs.
  cell "h3-max$x" "$(pkg minimax-h3 generate)" prompt="$(words 40000)" duration_s:=15 seed:=$((R + 911 + o))
  fits "h3-max$x"
}
run_inputs "" 0
# The capped pass: the LM-heavy cells --only names (all four when it names none), on a card over 20 GB.
GIB=$(grep -oE '· [0-9]+ GiB ·' machine.txt | head -1 | grep -oE '[0-9]+' || true)
INPUTS_ONLY=$ONLY INPUTS_CAP=,
for c in ph3-edit-max klein-max qwen2-max describer-max; do
  [ -n "$ONLY" ] && [[ $ONLY != *",$c,"* && $ONLY != *",$c-cap8,"* ]] || INPUTS_CAP+="$c-cap8,"
done
if [ "${GIB:-0}" -gt 20 ] && [ "$INPUTS_CAP" != , ]; then
  "$CLI" machine update "$M" --gpu-memory-limit 8GiB -y > inputs-cap8.log 2>&1 || log "inputs: no 8 GiB cap: $(tail -c 300 inputs-cap8.log)"
  ONLY=$INPUTS_CAP
  run_inputs -cap8 50
  "$CLI" machine update "$M" --gpu-memory-limit off -y > inputs-uncap.log 2>&1 || log "inputs: cap not lifted: $(tail -c 300 inputs-uncap.log)"
fi
ONLY=$INPUTS_ONLY
