# H3's video VAE at least as fast as ComfyUI's (cr-306). Paul, 11-16, on ComfyUI's "MiniMax H3's video VAE is ~2x faster":
# "if it's worthwhile, please incorporate it", "we should be using the --fast fp16_accumulation. Also for decode we should be
# using the int8 VAE where possible."
# minimax-h3's `vae_round_trip` on a 129-frame 1344x768 clip (ComfyUI's blog shape), int8 lane: a warm-up, then a timed call.
# - With cr-306's fused encoder (its plan reports `fused.minimaxh3videocausalconv3d`), its `vae_encode` and `vae_decode` are
#   held to ComfyUI master's on this SKU (h3-vae.tsv: comfy-kitchen #167's kernels, its int8 VAE, --fast fp16_accumulation, timed
#   on one pod by sessions/h3-vae.sh), each first scaled by the call's mean SM clock against that pod's (rentals of one SKU run at
#   451 to 576 W), within 5%. A fused encoder declared and not applied fails: the encoder regressed. A pod whose `read_video`
#   takes 3x the reference's is a slow host: its times are noted, not judged.
# - Without it (a Runtime or minimax-h3 before cr-306), the cell's times are only analyze.py's: its known-good ledger, a
#   no-regression bar with its own host calibration.
# A minimax-h3 with no `vae_round_trip` (before cr-306) is not run; a SKU with no row is noted.
# h3_vae_judge <run record>: the verdicts of one timed call (CPU-only: no cell, no rental).
h3_vae_judge() {
  local enc dec rd sm fused c_enc c_dec c_sm c_rd source part ours theirs scaled
  read -r enc dec rd sm fused < <(jq -r '[.measurements, .calls[]?.measurements] as $m | [$m[] | .attribution.stages? // {}] as $s
    | [$m[] | .execution.plan_reports? // {} | .[] | to_entries[] | select(.key | startswith("fused.minimaxh3videocausalconv3d"))
       | .value.applied] as $f
    | "\(([$s[].vae_encode.total_ms // empty] | add // 0) / 1000) \(([$s[].vae_decode.total_ms // empty] | add // 0) / 1000)"
      + " \(([$s[].read_video.total_ms // empty] | add // 0) / 1000) \([$m[] | .memory.driver[]?.sm_mhz_mean // empty] | first // 0)"
      + " \(if ($f | length) == 0 then "none" elif all($f[]; . == true) then "applied" else "unapplied" end)"' "$1")
  if [ "$fused" = none ]; then
    log "h3-vae: encode ${enc}s, decode ${dec}s on $SKU without cr-306's fused encoder: held to analyze.py's known-good, not ComfyUI's"
    return 0
  fi
  [ "$fused" = applied ] || { expect h3-vae-encode FAIL "cr-306's fused encoder was declared and not applied: the encoder regressed (vae_encode ${enc}s)"; return 0; }
  read -r _ c_enc c_dec c_sm c_rd source < <(awk -F'\t' -v s="$SKU" '$1 == s' "$HERE/suites/runtime/h3-vae.tsv")
  if [ -z "${c_enc:-}" ]; then log "h3-vae: encode ${enc}s, decode ${dec}s on $SKU; h3-vae.tsv has no ComfyUI time for it"; return 0; fi
  if awk -v r="$rd" -v c="$c_rd" 'BEGIN { exit !(r >= 3 * c) }'; then
    log "h3-vae: encode ${enc}s, decode ${dec}s at ${sm} MHz on a slow host (read_video ${rd}s against ${c_rd}s): noted, not judged"
    return 0
  fi
  [ "${sm:-0}" != 0 ] || sm=$c_sm  # no clock read: unscaled
  for part in encode decode; do
    if [ $part = encode ]; then ours=$enc theirs=$c_enc; else ours=$dec theirs=$c_dec; fi
    scaled=$(awk -v o="$ours" -v m="$sm" -v c="$c_sm" 'BEGIN { printf "%.2f", o * m / c }')
    expect "h3-vae-$part" "$(awk -v o="$scaled" -v t="$theirs" 'BEGIN { print (o > 0 && o <= t * 1.05) ? "PASS" : "FAIL" }')" \
      "vae_$part ${ours}s at ${sm} MHz, ${scaled}s at ${c_sm} MHz, against ComfyUI's ${theirs}s (bar x1.05) on $SKU, 129 frames at 1344x768 ($source)"
  done
}
if [ -z "$ONLY" ] || [[ $ONLY == *",h3-vae,"* || $ONLY == *",minimax-h3,"* ]]; then
  VAE_FN=$(pkg minimax-h3 vae_round_trip)
  if [[ $VAE_FN == ./* ]] && ! grep -qs 'def vae_round_trip' "$OUT/$(dirname "${VAE_FN#./}")/h3.py"; then
    log "h3-vae: this minimax-h3 has no vae_round_trip (before cr-306): not run"
  elif [[ $VAE_FN != ./* ]] && ! "$CLI" run "$VAE_FN" --describe > /dev/null 2>&1; then
    log "h3-vae: $VAE_FN has no vae_round_trip (a release before cr-306): not run"
  else
    VAE_CLIP=$OUT/h3-vae-129.mp4
    nice -n 19 ffmpeg -nostdin -v error -y -loop 1 -i "$HERE/ref/seed.jpg" -frames:v 129 -r 24 -pix_fmt yuv420p -c:v libx264 -crf 12 \
      -vf "scale=2688:1536:force_original_aspect_ratio=increase,crop=2688:1536,zoompan=z='1+0.0012*on':x='iw/2-iw/zoom/2':y='ih/2-ih/zoom/2':d=1:s=1344x768:fps=24" \
      "$VAE_CLIP"
    SAVED=$ONLY; [ -z "$ONLY" ] || ONLY="$ONLY,h3-vae-warmup,"
    cell h3-vae-warmup "$VAE_FN" --file video="$VAE_CLIP" --lane int8
    ONLY=$SAVED
    cell h3-vae "$VAE_FN" --file video="$VAE_CLIP" --lane int8
    if [ -f h3-vae.json ]; then
      if [ "$(run_get "$OUT" h3-vae .status)" != completed ]; then
        expect h3-vae FAIL "run $(cat h3-vae.number 2>/dev/null) $(run_get "$OUT" h3-vae .status)"
      else
        h3_vae_judge "$OUT/h3-vae.json"
      fi
    fi
  fi
fi
