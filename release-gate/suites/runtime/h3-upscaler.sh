# H3's latent upscaler on its int8 lane (cr-327). Paul, 11-17: "can this upscaler be int8 convrot? this is used in the
# upscaler for minimax h3?"; "try to use int-8-convrot wherever appropriate and possible".
# minimax-h3's `latent_upscale` on a 15 s clip's latents (release-gate/ref/seed.jpg in a push-in, 362 frames at 1344x768,
# encoded by `vae_round_trip`) to 1440p: a warm-up, then a timed call. The upscaler is the candidate's --models one when it
# names lbh-123-ai/h3-latent-upscaler, else minimax-h3's default.
# - Its blocks on int8 conv leaves (the plan reports `fused.conv-blocks.int8` applied): its `upscale_latent` is held to
#   h3-upscaler.tsv's on this SKU (sessions/h3-upscaler.sh), scaled by the call's SM clock against that pod's, within x1.25;
#   eager bf16 there takes about 2.6x. Declared and not applied for any reason but a 16-bit lane fails: the int8 path
#   regressed. A 16-bit lane is logged: which lane a card gets is the lane cell's to judge.
# - A Runtime or minimax-h3 without them (before cr-327) is logged, not judged.
# A minimax-h3 with no `latent_upscale` (before cr-327) is not run; a SKU with no row is noted.
# h3_upscaler_judge <run record>: the verdict of one timed call (CPU-only: no cell, no rental).
h3_upscaler_judge() {
  local up sm state reason c_up c_bf c_sm source scaled
  IFS=$'\t' read -r up sm state reason < <(jq -r '[.measurements, .calls[]?.measurements] as $m
    | [$m[] | .attribution.stages? // {}] as $s
    | [$m[] | .execution.plan_reports? // {} | .[] | to_entries[] | select(.key == "fused.conv-blocks.int8") | .value] as $f
    | [(([$s[].upscale_latent.total_ms // empty] | add // 0) / 1000 | tostring),
       ([$m[] | .memory.driver[]?.sm_mhz_mean // empty] | first // 0 | tostring),
       (if ($f | length) == 0 then "none" elif all($f[]; .applied == true) then "applied" else "unapplied" end),
       ([$f[] | .reason // empty] | first // "")] | @tsv' "$1")
  case $state in
    none)
      log "h3-upscaler: upscale_latent ${up}s on $SKU without cr-327's int8 conv blocks: logged, not judged"; return 0 ;;
    unapplied)
      if [[ $reason == "no int8 conv leaves"* ]]; then
        log "h3-upscaler: upscale_latent ${up}s on $SKU on a 16-bit lane ($reason): logged, the lane cell judges lanes"
      else
        expect h3-upscaler FAIL "cr-327's int8 conv blocks were declared and not applied ($reason): upscale_latent ${up}s"
      fi
      return 0 ;;
  esac
  IFS=$'\t' read -r _ c_up c_bf c_sm source < <(awk -F'\t' -v s="$SKU" '$1 == s' "$HERE/suites/runtime/h3-upscaler.tsv")
  if [ -z "${c_up:-}" ]; then log "h3-upscaler: upscale_latent ${up}s on $SKU; h3-upscaler.tsv has no time for it"; return 0; fi
  [ "${sm:-0}" != 0 ] || sm=$c_sm  # no clock read: unscaled
  scaled=$(awk -v o="$up" -v m="$sm" -v c="$c_sm" 'BEGIN { printf "%.3f", o * m / c }')
  expect h3-upscaler "$(awk -v o="$scaled" -v t="$c_up" 'BEGIN { print (o > 0 && o <= t * 1.25) ? "PASS" : "FAIL" }')" \
    "upscale_latent ${up}s at ${sm} MHz, ${scaled}s at ${c_sm} MHz, against int8's ${c_up}s (bar x1.25; eager bf16 ${c_bf}s) on $SKU, 107 latent frames to 1440p ($source)"
}
if [ -z "$ONLY" ] || [[ $ONLY == *",h3-upscaler,"* || $ONLY == *",minimax-h3,"* ]]; then
  UP_FN=$(pkg minimax-h3 latent_upscale) UP_RT=$(pkg minimax-h3 vae_round_trip)
  if [[ $UP_FN == ./* ]] && ! grep -qs 'def latent_upscale' "$OUT/$(dirname "${UP_FN#./}")/h3.py"; then
    log "h3-upscaler: this minimax-h3 has no latent_upscale (before cr-327): not run"
  elif [[ $UP_FN != ./* ]] && ! "$CLI" run "$UP_FN" --describe > /dev/null 2>&1; then
    log "h3-upscaler: $UP_FN has no latent_upscale (a release before cr-327): not run"
  else
    UP_CLIP=$OUT/h3-upscaler-362.mp4
    nice -n 19 ffmpeg -nostdin -v error -y -loop 1 -i "$HERE/ref/seed.jpg" -frames:v 362 -r 24 -pix_fmt yuv420p -c:v libx264 -crf 12 \
      -vf "scale=2688:1536:force_original_aspect_ratio=increase,crop=2688:1536,zoompan=z='1+0.0012*on':x='iw/2-iw/zoom/2':y='ih/2-ih/zoom/2':d=1:s=1344x768:fps=24" \
      "$UP_CLIP"
    UP_MODEL=(); for m in ${MODELS//,/ }; do [[ $m == lbh-123-ai/h3-latent-upscaler@* ]] && UP_MODEL=("model.latent_upscaler=$m"); done
    SAVED=$ONLY; [ -z "$ONLY" ] || ONLY="$ONLY,h3-upscaler-encode,h3-upscaler-warmup,h3-upscaler,"
    cell h3-upscaler-encode "$UP_RT" --file video="$UP_CLIP" --lane int8
    run_outputs "$OUT" h3-upscaler-encode > /dev/null
    UP_LAT=$(sed -n 's/^latents: //p' "$OUT/h3-upscaler-encode.export.err" 2> /dev/null | head -1)
    if [ -z "$UP_LAT" ] || [ ! -s "$UP_LAT" ]; then
      expect h3-upscaler FAIL "the clip's encode gave no latents (run $(cat h3-upscaler-encode.number 2>/dev/null))"
    else
      cell h3-upscaler-warmup "$UP_FN" --file latents="$UP_LAT" resolution=1440p "${UP_MODEL[@]}"
      cell h3-upscaler "$UP_FN" --file latents="$UP_LAT" resolution=1440p "${UP_MODEL[@]}"
      if [ -f h3-upscaler.json ]; then
        if [ "$(run_get "$OUT" h3-upscaler .status)" != completed ]; then
          expect h3-upscaler FAIL "run $(cat h3-upscaler.number 2>/dev/null) $(run_get "$OUT" h3-upscaler .status)"
        else
          h3_upscaler_judge "$OUT/h3-upscaler.json"
        fi
      fi
    fi
    ONLY=$SAVED
  fi
fi
