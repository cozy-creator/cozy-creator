#!/usr/bin/env bash
# The cheapest card with two GPUs listed now, per kernel family; a family with none is said on stderr, and is not tested.
set -euo pipefail
cozy --json machine rent | jq -r '
  def family: {"12.0": "sm120", "12.1": "sm120", "9.0": "sm90", "10.0": "sm100", "10.3": "sm100",
               "8.0": "older", "8.6": "older", "8.7": "older", "8.9": "older"}[.] // empty;
  [.[] | . as $card | (.compute_capability | family) as $family
    | .widths[]? | select(.accelerator_count == 2)
    | {family: $family, name: $card.name, usd: (.price_usd_micros_per_hour / 1000000)}] as $cards
  | ("sm120", "sm90", "older") as $family
  | ($cards | map(select(.family == $family)) | min_by(.usd)) as $best
  | if $best then "\($family) \($best.name) $\($best.usd)/h" else "\($family): no 2-GPU card listed\n" | stderr | empty end'
