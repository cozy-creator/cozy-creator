#!/usr/bin/env bash
# The cheapest card listed now per kernel family; a family with none is said on stderr, and is not tested.
set -euo pipefail
cozy --json machine rent | jq -r '
  def family: {"12.0": "sm120", "12.1": "sm120", "9.0": "sm90", "10.0": "sm100", "10.3": "sm100", "8.9": "sm89",
               "8.0": "sm86", "8.6": "sm86", "8.7": "sm86"}[.] // empty;
  [.[] | . as $card | (.compute_capability | family) as $family
    | select($family != null) | .widths[]? | select(.accelerator_count == 1)
    | {family: $family, name: $card.name, usd: (.price_usd_micros_per_hour / 1000000)}] as $cards
  | ("sm120", "sm90", "sm89", "sm86", "sm100") as $family
  | ($cards | map(select(.family == $family)) | min_by(.usd)) as $best
  | if $best then "\($family) \($best.name) $\($best.usd)/h" else "\($family): no card listed\n" | stderr | empty end'
