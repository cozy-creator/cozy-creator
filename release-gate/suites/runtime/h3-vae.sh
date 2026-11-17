# H3's video VAE at least as fast as ComfyUI's (cr-306). Paul, 11-16, on ComfyUI's "MiniMax H3's video VAE is ~2x faster":
# "if it's worthwhile, please incorporate it", "we should be using the --fast fp16_accumulation. Also for decode we should be
# using the int8 VAE where possible."
# minimax-h3's `vae_round_trip` on a 129-frame 1344x768 clip (ComfyUI's blog shape), int8 lane: a warm-up, then a timed call
# whose `vae_encode` and `vae_decode` stages must each be within 5% of ComfyUI's on this SKU (h3-vae.tsv: ComfyUI master,
# comfy-kitchen #167's kernels, its int8 VAE and --fast fp16_accumulation, timed on the same pod by sessions/h3-vae.sh). A SKU
# with no row runs and is noted.
if [ -z "$ONLY" ] || [[ $ONLY == *",h3-vae,"* || $ONLY == *",minimax-h3,"* ]]; then
  VAE_CLIP=$OUT/h3-vae-129.mp4
  nice -n 19 ffmpeg -nostdin -v error -y -loop 1 -i "$HERE/ref/seed.jpg" -frames:v 129 -r 24 -pix_fmt yuv420p -c:v libx264 -crf 12 \
    -vf "scale=2688:1536:force_original_aspect_ratio=increase,crop=2688:1536,zoompan=z='1+0.0012*on':x='iw/2-iw/zoom/2':y='ih/2-ih/zoom/2':d=1:s=1344x768:fps=24" \
    "$VAE_CLIP"
  VAE_FN=$(pkg minimax-h3 vae_round_trip)
  SAVED=$ONLY; [ -z "$ONLY" ] || ONLY="$ONLY,h3-vae-warmup,"
  cell h3-vae-warmup "$VAE_FN" --file video="$VAE_CLIP" --lane int8
  ONLY=$SAVED
  cell h3-vae "$VAE_FN" --file video="$VAE_CLIP" --lane int8
  if [ -f h3-vae.json ]; then
    read -r enc dec < <(run_get "$OUT" h3-vae '[.measurements, .calls[]?.measurements | .attribution.stages? // {}] as $s
      | "\(([$s[].vae_encode.total_ms // empty] | add // 0) / 1000) \(([$s[].vae_decode.total_ms // empty] | add // 0) / 1000)"')
    read -r _ c_enc c_dec source < <(awk -F'\t' -v s="$SKU" '$1 == s' "$HERE/suites/runtime/h3-vae.tsv")
    if [ "$(run_get "$OUT" h3-vae .status)" != completed ]; then
      expect h3-vae FAIL "run $(cat h3-vae.number 2>/dev/null) $(run_get "$OUT" h3-vae .status)"
    elif [ -z "${c_enc:-}" ]; then
      log "h3-vae: encode ${enc}s, decode ${dec}s on $SKU; h3-vae.tsv has no ComfyUI time for it"
    else
      for part in encode decode; do
        if [ $part = encode ]; then ours=$enc theirs=$c_enc; else ours=$dec theirs=$c_dec; fi
        expect "h3-vae-$part" "$(awk -v o="$ours" -v t="$theirs" 'BEGIN { print (o > 0 && o <= t * 1.05) ? "PASS" : "FAIL" }')" \
          "vae_$part ${ours}s against ComfyUI's ${theirs}s (bar x1.05) on $SKU, 129 frames at 1344x768 ($source)"
      done
    fi
  fi
fi
