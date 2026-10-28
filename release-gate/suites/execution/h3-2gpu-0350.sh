# Runtime 0.35.0 + minimax-h3 0.1.8 (master's H3 on the author API) pre-roll, 2-GPU SM120 (release lane, proof-only): ref2va
# Turbo 15 s context-parallel on both GPUs (the cell 0.34.12 crashed), fl2va Turbo on both GPUs, then ref2va Turbo on one GPU
# of the same pod, each with its own prompt and seed (both functions memoize). PASS: all complete, two ranks on two GPUs,
# one route for all.
. "$HERE/suites/execution/h3-route.sh"
H3REF="[{\"name\": \"$NAME\", \"kind\": \"character\", \"image\": \"$REF\"}]"
cell h3-2g "$(pkg minimax-h3 ref2va_turbo)" references:="$H3REF" seed:=$((R + 361)) duration_s:=15 resolution=768p prompt="<$NAME> $MOTION" --gpus=2
cell fl2va-2g "$(pkg minimax-h3 fl2va_turbo)" seed:=$((R + 363)) duration_s:=15 resolution=768p prompt="$CHAR. $MOTION" --gpus=2
cell h3-1g "$(pkg minimax-h3 ref2va_turbo)" references:="$H3REF" seed:=$((R + 362)) duration_s:=15 resolution=768p prompt="<$NAME> $MOTION Dusk falls." --gpus=1
if [ -f h3-2g.json ] && [ -f fl2va-2g.json ] && [ -f h3-1g.json ]; then
  ok=PASS r1=$(route h3-1g) msg=
  for c in h3-2g fl2va-2g h3-1g; do
    s=$(run_get "$OUT" "$c" .status); read -r deg ng <<< "$(degree "$c")"; r=$(route "$c")
    want=2; [ "$c" = h3-1g ] && want=1
    { [ "$s" = completed ] && [ "$deg" = "$want" ] && [ "$ng" = "$want" ] && [ -n "$r" ] && [ "$r" = "$r1" ]; } || ok=FAIL
    msg="$msg $c $s degree $deg on $ng GPUs: $r; served $(served "$c"); exact $(exact_sites "$c") ||"
  done
  expect h3-2gpu-route $ok "$msg"
fi
