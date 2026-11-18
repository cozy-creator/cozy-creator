# H3's video VAE at least as fast as ComfyUI's (cr-306). Paul, 11-16, on ComfyUI's "MiniMax H3's video VAE is ~2x faster":
# "if it's worthwhile, please incorporate it", "we should be using the --fast fp16_accumulation. Also for decode we should be
# using the int8 VAE where possible."
# minimax-h3's `vae_round_trip` on a 129-frame 1344x768 clip (ComfyUI's blog shape), int8 lane: a warm-up, then a timed call.
# - With cr-306's fused encoder (its plan reports `fused.minimaxh3videocausalconv3d`), its `vae_encode` and `vae_decode` are
#   held to ComfyUI master's on this SKU (comfy-kitchen #167's kernels, its int8 VAE, --fast fp16_accumulation, timed on one pod
#   by sessions/h3-vae.sh) within 5%, ComfyUI's time first carried to this card (one SKU's rentals run at 451 to
#   576 W). The encode is carried by the call's mean SM clock against that pod's. The decode takes the slower card of two
#   estimates: that clock, and this call's encode against ours on ComfyUI's pod (`ours_encode_s`). The clock understates a
#   capped card's slowdown (senshi, 492 W: 8% by clock, 16% by its encode, our decode 17%); a faster encoder lowers the
#   second, and a slower one fails its own check. A fused encoder declared and not applied fails: the encoder regressed.
#   Noted, not judged: a pod whose `read_video` takes 3x the reference's (a slow host), and a card whose SM clock limit moved
#   between the warm-up and the timed call (its host retuned it: utahime, 3120 then 3465 MHz, decoded 31% slower than its
#   encode predicts, gate 44187736024; six other pods agree within 6%).
# - Without it (a Runtime or minimax-h3 before cr-306), the cell's times are only analyze.py's: its known-good ledger, a
#   no-regression bar with its own host calibration.
# ComfyUI's times are private (Paul 11-10: its numbers stay off public places until every cell is won): the benchmarks repo's
# refs/h3-vae.tsv, which release-gate.yml checks out at comfyui.sh's BENCHMARKS_AT; h3-vae.tsv here holds only Cozy's calls on
# that pod. A verdict says ComfyUI's bar, never its number. Without the checkout, or a SKU with no row, the times are noted.
# A minimax-h3 with no `vae_round_trip` (before cr-306) is not run.
H3_VAE_REFS=${BENCHMARKS_DIR:-/nonexistent}/refs/h3-vae.tsv
# h3_vae_judge <run record>: the verdicts of one timed call, beside its warm-up's record (CPU-only: no cell, no rental).
h3_vae_judge() {
  local enc dec rd sm lim fused c_enc c_dec c_sm c_rd c_own source scaled by_clock by_encode here warm lim_w
  local pick='[.measurements, .calls[]?.measurements] as $m | [$m[] | .memory.driver[]?.sm_mhz_limit // empty] | first // 0'
  read -r enc dec rd sm lim fused < <(jq -r '[.measurements, .calls[]?.measurements] as $m | [$m[] | .attribution.stages? // {}] as $s
    | [$m[] | .execution.plan_reports? // {} | .[] | to_entries[] | select(.key | startswith("fused.minimaxh3videocausalconv3d"))
       | .value.applied] as $f
    | "\(([$s[].vae_encode.total_ms // empty] | add // 0) / 1000) \(([$s[].vae_decode.total_ms // empty] | add // 0) / 1000)"
      + " \(([$s[].read_video.total_ms // empty] | add // 0) / 1000) \([$m[] | .memory.driver[]?.sm_mhz_mean // empty] | first // 0)"
      + " \([$m[] | .memory.driver[]?.sm_mhz_limit // empty] | first // 0)"
      + " \(if ($f | length) == 0 then "none" elif all($f[]; . == true) then "applied" else "unapplied" end)"' "$1")
  if [ "$fused" = none ]; then
    log "h3-vae: encode ${enc}s, decode ${dec}s on $SKU without cr-306's fused encoder: held to analyze.py's known-good, not ComfyUI's"
    return 0
  fi
  [ "$fused" = applied ] || { expect h3-vae-encode FAIL "cr-306's fused encoder was declared and not applied: the encoder regressed (vae_encode ${enc}s)"; return 0; }
  read -r _ c_sm c_rd c_own source < <(awk -F'\t' -v s="$SKU" '$1 == s' "$HERE/suites/runtime/h3-vae.tsv")
  read -r _ c_enc c_dec _ < <(awk -F'\t' -v s="$SKU" '$1 == s' "$H3_VAE_REFS" 2> /dev/null)
  if [ -z "${c_enc:-}" ] || [ -z "${c_sm:-}" ]; then log "h3-vae: encode ${enc}s, decode ${dec}s on $SKU; no ComfyUI time for it (refs/h3-vae.tsv)"; return 0; fi
  if awk -v r="$rd" -v c="$c_rd" 'BEGIN { exit !(r >= 3 * c) }'; then
    log "h3-vae: encode ${enc}s, decode ${dec}s at ${sm} MHz on a slow host (read_video ${rd}s against ${c_rd}s): noted, not judged"
    return 0
  fi
  warm=$(dirname "$1")/h3-vae-warmup.json
  lim_w=$( { [ -f "$warm" ] && jq -r "$pick" "$warm"; } 2> /dev/null || echo 0)
  if [ "${lim_w:-0}" != 0 ] && [ "${lim:-0}" != 0 ] && [ "$lim_w" != "$lim" ]; then
    log "h3-vae: encode ${enc}s, decode ${dec}s at ${sm} MHz on a card its host retuned mid-session (SM clock limit ${lim_w} MHz at the warm-up, ${lim} MHz timed): noted, not judged"
    return 0
  fi
  [ "${sm:-0}" != 0 ] || sm=$c_sm  # no clock read: unscaled
  scaled=$(awk -v o="$enc" -v m="$sm" -v c="$c_sm" 'BEGIN { printf "%.2f", o * m / c }')
  expect h3-vae-encode "$(awk -v o="$scaled" -v t="$c_enc" 'BEGIN { print (o > 0 && o <= t * 1.05) ? "PASS" : "FAIL" }')" \
    "vae_encode ${enc}s at ${sm} MHz, ${scaled}s at ${c_sm} MHz, against ComfyUI's time (bar x1.05, refs/h3-vae.tsv) on $SKU, 129 frames at 1344x768"
  read -r by_clock by_encode here < <(awk -v t="$c_dec" -v m="$sm" -v c="$c_sm" -v e="$enc" -v o="$c_own" 'BEGIN {
    k = t * c / m; n = o > 0 ? t * e / o : 0; printf "%.2f %.2f %.2f", k, n, (n > k ? n : k) }')
  expect h3-vae-decode "$(awk -v d="$dec" -v h="$here" 'BEGIN { print (d > 0 && d <= h * 1.05) ? "PASS" : "FAIL" }')" \
    "vae_decode ${dec}s against ComfyUI's time carried to this card (bar x1.05; the slower of its SM clock, ${sm} against ${c_sm} MHz, and this call's encode, ${enc}s against our ${c_own}s there) on $SKU, 129 frames at 1344x768"
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
# A picture through the same encoder (cr-323): minimax-h3's `vae_encode_picture` encodes one as ref2va encodes a picture
# reference at `high` fidelity, one frame at a 2048 px short edge (release-gate/ref/seed.jpg, 2048x2048), a warm-up then
# a timed call. A lone frame meets only each causal conv's last temporal tap.
# - A Runtime whose plan reports `"lone_frame": "last_tap"` runs that tap alone: its `vae_encode` is held to
#   h3-vae-picture.tsv's on this SKU (that path's own time), scaled by the call's SM clock against that pod's, within x1.5.
#   All three taps take about 2.3x (pod-session 44172763099). A slow host is h3-vae's own (that call's `read_video` at 3x
#   h3-vae.tsv's): noted, not judged. The picture's own `read_image` (a Lanczos resize) is no measure of one: 3.7x the
#   reference's on a pod whose `read_video` was 2.3x (release-gate 44180357050).
# - A Runtime without it (before cr-323), or a minimax-h3 without the fused encoder, is logged, not judged.
# A minimax-h3 with no `vae_encode_picture` (before cr-323) is not run.
# h3_vae_picture_judge <run record> [h3-vae's run record]: the verdict of one timed call (CPU-only: no cell, no rental).
h3_vae_picture_judge() {
  local enc sm path c_enc c_sm source scaled rd c_rd
  read -r enc sm path < <(jq -r '[.measurements, .calls[]?.measurements] as $m | [$m[] | .attribution.stages? // {}] as $s
    | [$m[] | .execution.plan_reports? // {} | .[] | to_entries[] | select(.key | startswith("fused.minimaxh3videocausalconv3d"))
       | .value] as $f
    | "\(([$s[].vae_encode.total_ms // empty] | add // 0) / 1000) \([$m[] | .memory.driver[]?.sm_mhz_mean // empty] | first // 0)"
      + " \(if ($f | length) == 0 or any($f[]; .applied != true) then "eager"
            elif all($f[]; .lone_frame == "last_tap") then "last_tap" else "three_taps" end)"' "$1")
  if [ "$path" != last_tap ]; then
    log "h3-vae-picture: encode ${enc}s on $SKU, its encoder $path (no cr-323 lone-frame path): logged, not judged"
    return 0
  fi
  read -r _ c_enc c_sm source < <(awk -F'\t' -v s="$SKU" '$1 == s' "$HERE/suites/runtime/h3-vae-picture.tsv")
  if [ -z "${c_enc:-}" ]; then log "h3-vae-picture: encode ${enc}s on $SKU; h3-vae-picture.tsv has no time for it"; return 0; fi
  if [ -f "${2:-}" ]; then
    rd=$(jq -r '[.measurements, .calls[]?.measurements] | map(.attribution.stages?.read_video.total_ms // empty) | (add // 0) / 1000' "$2")
    read -r _ _ c_rd _ < <(awk -F'\t' -v s="$SKU" '$1 == s' "$HERE/suites/runtime/h3-vae.tsv")
    if [ -n "${c_rd:-}" ] && awk -v r="$rd" -v c="$c_rd" 'BEGIN { exit !(r >= 3 * c) }'; then
      log "h3-vae-picture: encode ${enc}s at ${sm} MHz on a slow host (h3-vae's read_video ${rd}s against ${c_rd}s): noted, not judged"
      return 0
    fi
  fi
  [ "${sm:-0}" != 0 ] || sm=$c_sm  # no clock read: unscaled
  scaled=$(awk -v o="$enc" -v m="$sm" -v c="$c_sm" 'BEGIN { printf "%.3f", o * m / c }')
  expect h3-vae-picture "$(awk -v o="$scaled" -v t="$c_enc" 'BEGIN { print (o > 0 && o <= t * 1.5) ? "PASS" : "FAIL" }')" \
    "vae_encode ${enc}s at ${sm} MHz, ${scaled}s at ${c_sm} MHz, against the last tap's ${c_enc}s (bar x1.5) on $SKU, one 2048x2048 picture ($source)"
}
if [ -z "$ONLY" ] || [[ $ONLY == *",h3-vae,"* || $ONLY == *",h3-vae-picture,"* || $ONLY == *",minimax-h3,"* ]]; then
  PIC_FN=$(pkg minimax-h3 vae_encode_picture)
  if [[ $PIC_FN == ./* ]] && ! grep -qs 'def vae_encode_picture' "$OUT/$(dirname "${PIC_FN#./}")/h3.py"; then
    log "h3-vae-picture: this minimax-h3 has no vae_encode_picture (before cr-323): not run"
  elif [[ $PIC_FN != ./* ]] && ! "$CLI" run "$PIC_FN" --describe > /dev/null 2>&1; then
    log "h3-vae-picture: $PIC_FN has no vae_encode_picture (a release before cr-323): not run"
  else
    SAVED=$ONLY; [ -z "$ONLY" ] || ONLY="$ONLY,h3-vae-picture-warmup,h3-vae-picture,"
    cell h3-vae-picture-warmup "$PIC_FN" --file image="$HERE/ref/seed.jpg" --lane int8
    cell h3-vae-picture "$PIC_FN" --file image="$HERE/ref/seed.jpg" --lane int8
    ONLY=$SAVED
    if [ -f h3-vae-picture.json ]; then
      if [ "$(run_get "$OUT" h3-vae-picture .status)" != completed ]; then
        expect h3-vae-picture FAIL "run $(cat h3-vae-picture.number 2>/dev/null) $(run_get "$OUT" h3-vae-picture .status)"
      else
        h3_vae_picture_judge "$OUT/h3-vae-picture.json" "$OUT/h3-vae.json"
      fi
    fi
  fi
fi
