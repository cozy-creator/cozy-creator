# Prompt LMs after other models (escaped in Runtime 0.45.0 / TensorFS 0.24.1; Runtime 0.45.1 / TensorFS 0.24.2 fixed it).
# An LM's decode captures draw on a pool prepared before each capture, where nothing can be reclaimed. A grown pool was
# mapped as a second segment that no block spans (var-017), so Klein's upsampler after SDXL on a 4090 failed
# `capture_capacity` (16 MiB, gate 43117439338), and P-H3-Image's edit of one ~1 MP picture with its upsampler (window
# 2048 of ~1,100 prompt tokens) failed in 0.9 s on a 5090 (46479) and on a 4090 (46574, cr-279). The bench ran its
# prompt LMs off, so no gate before the release decoded one. Here, as 0.45.0 failed it on a 4090 (47036, 47038): P-H3-Image's
# upsampled text-to-image, then its upsampled edit on that executor (the pool grows to the edit's window), then SDXL, Klein
# and P-H3-Image again with their upsamplers on that card. Each completes and no decode capture is refused (no token decoded
# eagerly for want of room) with no Runtime retry: a fallback that completes still fails the cell. Every LM request is its
# own text, new each gate (`take $R`): an LM call with the same inputs is answered from its memo and decodes nothing
# (the 0.45.1 gate's 47055).
if [ -z "$ONLY" ] || [[ $ONLY == *",prompt-lm,"* ]]; then
  "$CLI" machine update "$M" --gpu-memory-limit off -y > prompt-lm-restart.log 2>&1
  plm_photo=$OUT/prompt-lm-photo.jpg
  [ -f "$plm_photo" ] || nice -n 19 ffmpeg -v error -y -i "$REF" -vf "scale=w='trunc(sqrt(1048576*iw/ih)/32)*32':h=-2" -q:v 3 "$plm_photo"
  plm_refs="[{\"name\":\"photo\",\"image\":\"$plm_photo\"}]"
  plm_cell() { local c=$1; shift; ONLY=${ONLY:+$ONLY,$c,} cell "$c" "$@"; }
  plm_cell prompt-lm-ph3-t2i "$(pkg p-h3-image generate)" prompt_upsampling:=true seed:=$((R + 459)) \
    instructions="A lighthouse keeper's kitchen at dawn, copper pots, steam, $OBJECT on the table (take $R)"
  plm_cell prompt-lm-ph3-edit "$(pkg p-h3-image generate)" references:="$plm_refs" prompt_upsampling:=true seed:=$((R + 460)) \
    instructions="<photo> make her hair light brunette pastel pink, she holds a small pistol in her right hand. Get rid of the orange blob at the bottom left (take $R)"
  plm_cell prompt-lm-sdxl "$(pkg sdxl generate)" prompt="$OBJECT in $SCENE, ink and watercolor" seed:=$((R + 461))
  plm_cell prompt-lm-klein-after "$(pkg flux.3-klein generate_turbo)" prompt="$CHAR (take $R)" prompt_upsampling:=true seed:=$((R + 462))
  plm_cell prompt-lm-ph3-after "$(pkg p-h3-image generate)" references:="$plm_refs" prompt_upsampling:=true seed:=$((R + 463)) \
    instructions="<photo> at night under neon signs, everything else kept (take $R)"
  plm=() plmbad=()
  for c in prompt-lm-ph3-t2i prompt-lm-ph3-edit prompt-lm-klein-after prompt-lm-ph3-after; do
    [ -f "$c.number" ] || { plmbad+=("$c: no run"); continue; }
    s=$(run_get "$OUT" "$c" .status)
    [ "$s" = completed ] || { plmbad+=("$c run $(cat "$c.number") $s $(run_get "$OUT" "$c" '(.reason.code // "") + " " + (.reason.message // "")' | head -c 160)"); continue; }
    lm=$(jq -c '[.measurements.residency.graphs // {}, (.calls[]?.measurements.residency.graphs // {}) | to_entries[]
      | select(.value.decode) | {lm: .key, captures: .value.decode.captures, refused: (.value.decode.refused | length)}]' "$c.json")
    re=$(jq '[.measurements.memory.stats.varena[]?.stats, (.calls[]?.measurements.memory.stats.varena[]?.stats)
      | (.retries // 0) + (.finer_splits // 0)] | add // 0' "$c.json")
    plm+=("$c run $(cat "$c.number") $lm retries $re")
    jq -e 'length > 0' <<< "$lm" > /dev/null || plmbad+=("$c: no prompt LM decoded")
    jq -e 'all(.refused == 0)' <<< "$lm" > /dev/null || plmbad+=("$c: a decode capture refused: $lm")
    [ "$re" = 0 ] || plmbad+=("$c: $re Runtime retries")
  done
  expect prompt-lm-capture "$([ ${#plmbad[@]} -eq 0 ] && echo PASS || echo FAIL)" \
    "${plmbad[*]:-every request completed, no decode capture refused, no Runtime retry}; ${plm[*]}"
fi
