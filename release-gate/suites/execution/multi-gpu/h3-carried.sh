# A ~26 s shot of two segments over two GPUs, the second carrying the first's last 56 frames (cr-288): each segment's
# steps track the rows its DiT attends. Segment 2 attends ~5% more rows than segment 1 (its window, the carried frames as
# condition rows, a second picture); on 2x H100 with Runtime 0.45.1 its Sol steps ran 1.6x segment 1's (Paul's 47448:
# 13.5 vs 8.0 s), the carried rows making the protected prefix ~19% of the sequence, which Sol's BF16 tiles paid twice.
source "$HERE/suites/execution/multi-gpu/h3-group.sh"
# h3_carried <cell> <inputs...>: h3_group's checks, then each segment's mean step after its first, over its attended rows
# squared (each call's exchanged bytes, which Ulysses makes linear in its rows), at most 1.12x segment 1's. 2x H100 SXM
# (asirpa): Runtime 0.45.1 1.18 (47621), rc/0.25.0 1.17 (47613), with cr-288's fix 1.08 (47736); Paul's 47448 1.19. The
# rest above 1.0 is the protected prefix's own rows, dense by NVIDIA's policy: ~16% of segment 2's rows, ~3% of segment 1's.
h3_carried() {
  local n=$1 seen ok; shift
  h3_group "$n" 600 carried_frames:=56 "$@"
  [ -f "$n.json" ] || return 0
  seen=$(jq -c '[.calls[]? | .measurements as $m | select($m.attribution.steps.denoise.device.series)
    | {bytes: ($m.execution.transport.exchange.crossed_gb // 0), steps: $m.attribution.steps.denoise.device.series[1:]}
    | .mean = (.steps | add / length)] as $s | ($s[0] // {}) as $first
    | {segments: [$s[] | {gb: .bytes, mean_ms: (.mean | round),
        per_rows: (if $first.bytes > 0 and .bytes > 0 then (.mean / $first.mean) / ((.bytes / $first.bytes) | . * .) else null end)}]}' \
    "$n.json" 2>/dev/null)
  ok=PASS
  jq -e '(.segments | length) == 2 and all(.segments[]; .per_rows != null and .per_rows <= 1.12)' <<< "$seen" >/dev/null || ok=FAIL
  expect "$n-steps" $ok "mean step after the first over (rows / segment 1's)^2, against segment 1's: $seen"
}
h3_carried h3-u2-carried seed:=$((R + 356)) "beats:=$(jq -cn --arg a "<$NAME> $MOTION" \
  --arg b 'The light changes as evening falls and the camera keeps moving around the same place; the sound carries on.' \
  '[{duration: 15, description: $a}, {duration: 11, description: $b}]')"
