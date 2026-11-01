# --cells file: se-087's reproducer (h3-shot-40s's request at seed 3006627215, where 0.2.2's run 24525 reframed at 29.25 s),
# judged at its piece boundaries as h3-shot-40s is.
cell se087-repro "$(pkg minimax-h3 shot)" --input /home/fidika/cozy_v2/outputs/opus-takeover-20261027/review/se087/requests/r0.json
if [ -f se087-repro.number ] && [ "$(run_get "$OUT" se087-repro .status)" = completed ]; then
  b=$("$CLI" run show "$(cat se087-repro.number)" --once 2>/dev/null | grep -o 'boundaries_s=[0-9.,]*' | head -1 | cut -d= -f2)
  r=$([ -n "$b" ] && "${FR[@]}" seams "$(video_of se087-repro)" --at "$b" 2>&1 | tail -1)
  expect se087-repro-seams "$(jq -r 'if (.boundaries | length) >= 2 and ((([0.6, 0.9 * (.inside_ssim // 1)] | min) as $bar | [.boundaries[] | .ratio <= 3 and .ssim >= $bar] | all)) then "PASS" else "FAIL" end' <<< "$r" 2>/dev/null || echo FAIL)" "boundaries ${b:-unread}: $r"
fi
