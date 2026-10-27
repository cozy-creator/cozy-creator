# Two entry points of one package on one GPU, the second run while the first's executor idles with its weights
# resident (19472/19480: idle H3 groups of one package kept their copies while the active one sat at the card
# limit, stalling 26-96 s per segment). Sourced by gate.sh --cells on a one-GPU rental; `only` picks the pairs
# (idle-klein-a,idle-klein-b,idle-klein-a2,idle-klein-b2 and idle-h3-t,idle-h3-lf). idle_entrypoints.py judges:
#   idle-klein-b, -b2  the other entrypoint's idle executor offloaded before this one's first kernel, this one
#                      stayed under the card limit and read none of its weights twice (each fits alone); b's
#                      peak is unknown (first use), b2's measured
#   idle-klein-a2, -b2 each again on its first executor: offloaded, not ended
#   idle-h3-lf         long_form's segment on ref2va_turbo's executor: one construction serves both
KA=$(pkg flux-3-klein generate) KB=$(pkg flux-3-klein generate_turbo)
# The bf16 lane (17.6 GiB of weights each): one fits a 24 GiB card, two never fit 32 GiB.
klein() { cell "$1" "$2" prompt="$3" prompt_upsampling:=false megapixels:=1 seed:=$((R + $4)) --lane bf16 --gpus=1; }
klein idle-klein-a "$KA" "$CHAR, studio portrait" 401
klein idle-klein-b "$KB" "$SCENE, photo" 402
klein idle-klein-a2 "$KA" "$OBJECT on a wooden table, product photo" 403
klein idle-klein-b2 "$KB" "$CHAR in $SCENE, film still" 406
# One supplied image reference, neither cleaned nor described: no model but H3's runs.
ln -sfn "$REF" "idle-ref.${REF##*.}"
lead="references: [{name: Lead, kind: character, image: ./idle-ref.${REF##*.}}]
clean_references: false
describe_references: false"
printf '%s\nprompt: "<Lead> %s The camera drifts left."\nduration_s: 15\nresolution: 768p\nseed: %d\n' \
  "$lead" "$MOTION" $((R + 404)) > idle-ref2va-turbo.yaml
printf '%s\nsegments: [{prompt: "<Lead> %s A slow push in.", duration_s: 15, seed: %d}]\n' \
  "$lead" "$MOTION" $((R + 405)) > idle-long-form.yaml
cell idle-h3-t "$(pkg minimax-h3 ref2va_turbo)" --input idle-ref2va-turbo.yaml --gpus=1
cell idle-h3-lf "$(pkg minimax-h3 long_form)" --input idle-long-form.yaml --gpus=1
"$CLI" machine logs "$M" memory -n 5000 > idle-memory.log 2>&1
cap=$("$CLI" --json machine show "$M" 2>/dev/null | jq '.status.gpus[0].memory_bytes // 0')
python3 -I "$HERE/suites/tensord/idle_entrypoints.py" . idle-memory.log "$cap" > idle-entrypoints.tsv
while IFS=$'\t' read -r name verdict detail; do expect "$name" "$verdict" "$detail"; done < idle-entrypoints.tsv
