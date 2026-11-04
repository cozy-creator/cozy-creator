# Suite `kernels`: every kernel the Runtime ships, once, on this card, through its real launch path, against a slow
# reference (README.md). The inventory cell reads what the card claims from the code, so a new kernel with no exerciser
# fails its own cell; check.py fails an entry no cell launched. On a card with two GPUs each runs again as GPU 1, what a
# group's follower is. Paul 11-04: "prior to a release, all of our kernels should be exercised at least once".
KD=$HERE/suites/kernels
if [ -n "$PKGS" ]; then
  python3 -I "$KD/ci_kernels/static_check.py" "$PKGS" > static-check.txt 2>&1 \
    && expect kernels-static PASS "$(tail -n 1 static-check.txt)" || expect kernels-static FAIL "$(tail -n 3 static-check.txt)"
fi
# minimax-h3's glue, the one package whose kernels are its own, and what it imports, from the packages this gate is for.
rm -rf "$KD/ci_kernels/_h3"
if [ -n "$PKGS" ]; then
  mkdir -p "$KD/ci_kernels/_h3"; : > "$KD/ci_kernels/_h3/__init__.py"
  for f in glue adaln_pruned overlay; do cp "$PKGS/minimax-h3/minimax_h3/$f.py" "$KD/ci_kernels/_h3/"; done
else
  log "kernels: no --packages, so minimax-h3's glue has no cell"
fi
cell kernels-inventory "$KD/inventory" seed:=$((R + 400)) --gpus=1
kernels_list=$(run_get "$OUT" kernels-inventory .result.detail | jq -r '.names[]' 2>/dev/null)
[ -n "$kernels_list" ] || expect kernels-inventory FAIL "no inventory: $(run_get "$OUT" kernels-inventory .status) $(run_get "$OUT" kernels-inventory .reason.message)"
kernels_seed=$((R + 401))
for kernel in $kernels_list; do
  cell "kernel-${kernel//:/-}" "$KD/gpu" kernel="$kernel" seed:=$((kernels_seed++)) --gpus=1
done
if [ "$("$CLI" --json machine show "$M" | jq '.status.gpus | length')" -ge 2 ]; then
  for kernel in $kernels_list; do
    cell "kernel1-${kernel//:/-}" "$KD/gpu" kernel="$kernel" index:=1 seed:=$((kernels_seed++)) --gpus=2
  done
fi
# The largest a public function accepts, through it: Regenerate-2K at 1440p from a plate the length of H3's longest shot.
# It reached 2^31 elements on one GPU, and the h3 suite's 5 s regen cells hid that for four releases. One card a gate, on
# 24 GB or more: about 30 min. Only `--only kernels-largest` runs it where a plain inventory pass is what's asked for.
if [[ $ONLY == *",kernels-largest,"* ]]; then
  nice -n 19 ffmpeg -v error -y -f lavfi -i testsrc2=size=1344x768:rate=24:duration=15 -c:v libx264 -pix_fmt yuv420p kernels-plate.mp4
  cell kernels-largest "$(pkg minimax-h3 upscale)" --file video="$OUT/kernels-plate.mp4" resolution=1440p \
    prompt="a test pattern with a moving sweep and a running timecode" seed:=$((R + 490)) 'upscale:={"model": "regenerate-2k"}' --gpus=1
fi
