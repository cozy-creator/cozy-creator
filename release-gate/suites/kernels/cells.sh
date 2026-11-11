# Suite `kernels`: every kernel the Runtime ships, once, on this card, through its real launch path, against a slow
# reference (README.md). The exercisers are the Runtime's own built-ins (`cozy_runtime.bench`: kernels_inventory,
# kernel_check), which drive/ calls: a package can't import the Runtime's internals (the package contract, 0.43.1). The
# inventory reads what the card claims from the code, so a new kernel with no exerciser fails its own cell; check.py fails
# an entry no cell launched. On a card with two GPUs each runs again as GPU 1, what a group's follower is. Paul 11-04:
# "prior to a release, all of our kernels should be exercised at least once".
KD=$HERE/suites/kernels
# Every launch against its kernel, read from the source here: the Runtime the machine runs, and --packages.
kernels_whl=$RWF
if [ -z "$kernels_whl" ]; then
  kernels_whl=$OUT/kernels-runtime.whl
  curl -fsSL --retry 5 -o "$kernels_whl" \
    "https://github.com/cozy-creator/cozy-runtime/releases/download/v$rt/cozy_runtime-$rt-cp312-abi3-manylinux_2_28_x86_64.whl" || kernels_whl=
fi
rm -rf "$OUT/kernels-runtime"
if [ -n "$kernels_whl" ] && unzip -q "$kernels_whl" 'cozy_runtime/*' -d "$OUT/kernels-runtime"; then
  python3 -I "$KD/static_check.py" "$OUT/kernels-runtime" ${PKGS:+"$PKGS"} > static-check.txt 2>&1 \
    && expect kernels-static PASS "$(tail -n 1 static-check.txt)" || expect kernels-static FAIL "$(tail -n 3 static-check.txt)"
else
  expect kernels-static FAIL "no source of Runtime $rt to read"
fi
cell kernels-inventory "$KD/drive/inventory" --gpus=1
kernels_list=$(run_get "$OUT" kernels-inventory .result.detail | jq -r '.names[]' 2>/dev/null)
[ -n "$kernels_list" ] || [ -n "$ONLY" ] || expect kernels-inventory FAIL "no inventory: $(run_get "$OUT" kernels-inventory .status) $(run_get "$OUT" kernels-inventory .reason.message)"
kernels_seed=$((R + 401))
for kernel in $kernels_list; do
  cell "kernel-${kernel//:/-}" "$KD/drive/gpu" kernel="$kernel" seed:=$((kernels_seed++)) --gpus=1
done
if [ "$("$CLI" --json machine show "$M" | jq '.status.gpus | length')" -ge 2 ]; then
  for kernel in $kernels_list; do
    cell "kernel1-${kernel//:/-}" "$KD/drive/gpu" kernel="$kernel" index:=1 seed:=$((kernels_seed++)) --gpus=2
  done
fi
# The largest a public function accepts, through it: Regenerate-2K at 1440p from a plate the length of H3's longest shot.
# It reached 2^31 elements on one GPU, and the h3 suite's 5 s regen cells hid that for four releases. One card a gate, on
# 24 GB or more: about 30 min. Only `--only kernels-largest` runs it where a plain inventory pass is what's asked for.
if [[ $ONLY == *",kernels-largest,"* ]]; then
  nice -n 19 ffmpeg -v error -y -f lavfi -i testsrc2=size=1344x768:rate=24:duration=15 -c:v libx264 -pix_fmt yuv420p kernels-plate.mp4
  cell kernels-largest "$(pkg minimax-h3 upscale)" to=1440p --gpus=1 \
    video="$(box kernels_plate "$OUT/kernels-plate.mp4" "a test pattern with a moving sweep and a running timecode")"
fi
