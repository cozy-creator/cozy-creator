# Runtime 0.34.12 + minimax-h3 0.1.5 pre-roll, 2-GPU SM120 (release lane, proof-only): H3 Turbo 15 s context-parallel on
# both GPUs (#1840 crashed it with minimax-h3 0.1.4), then on one GPU of the same pod; each with its own prompt and seed
# (fl2va/ref2va memoize). PASS: both complete, two ranks on two GPUs, and the 2-GPU run serves the 1-GPU route.
. "$HERE/suites/execution/h3-route.sh"
H=$(pkg minimax-h3 ref2va_turbo) H3REF="[{\"name\": \"$NAME\", \"kind\": \"character\", \"image\": \"$REF\"}]"
cell h3-2g "$H" references:="$H3REF" seed:=$((R + 361)) duration_s:=15 resolution=768p prompt="<$NAME> $MOTION" --gpus=2
cell h3-1g "$H" references:="$H3REF" seed:=$((R + 362)) duration_s:=15 resolution=768p prompt="<$NAME> $MOTION Dusk falls." --gpus=1
if [ -f h3-2g.json ] && [ -f h3-1g.json ]; then
  s2=$(run_get "$OUT" h3-2g .status) s1=$(run_get "$OUT" h3-1g .status); read -r deg ng <<< "$(degree h3-2g)"
  r2=$(route h3-2g) r1=$(route h3-1g)
  ok=PASS; { [ "$s2" = completed ] && [ "$s1" = completed ] && [ "$deg" = 2 ] && [ "$ng" = 2 ] && [ -n "$r2" ] && [ "$r2" = "$r1" ]; } || ok=FAIL
  expect h3-2gpu-route $ok "2 GPUs $s2 (degree $deg on $ng GPUs): $r2; served $(served h3-2g); exact $(exact_sites h3-2g) || 1 GPU $s1: $r1; served $(served h3-1g); exact $(exact_sites h3-1g)"
fi
