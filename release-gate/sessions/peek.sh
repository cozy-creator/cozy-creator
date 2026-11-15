# peek: rents nothing and ends nothing. This account's live rentals (SKU, state, price) and each one's `machine show`,
# read in a job of its own: a long session's job log is closed until it ends, and a paid pod's progress must be checked
# while it runs (cr-297). Run it with rent false. params: none.
. "$(dirname "$0")/lib.sh"
cozy machine ls --json > "$OUT/ls.json" 2>&1 || fail "machine ls: $(tail -c 300 "$OUT/ls.json")"
mapfile -t live < <(jq -r '.machines[]? | select(.state != "ended" and .rental_id != null)
  | "\(.name) \(.rental_id) \(.state) \(.rental.sku // "?") \(.rental.usd_micros_per_hour // "?")"' "$OUT/ls.json")
say "${#live[@]} live rentals"
for row in "${live[@]}"; do
  read -r name rid st sku price <<<"$row"
  say "rental $name $rid $st $sku $price micro-USD/h"
  cozy machine show "$name" > "$OUT/show-$name.txt" 2>&1
  while IFS= read -r line; do say "  $line"; done < <(head -80 "$OUT/show-$name.txt")
  cozy machine show "$name" --json > "$OUT/show-$name.json" 2>&1
  say "  limits: $(jq -c '[paths(scalars) as $p | select($p | map(tostring) | join(".") | test("limit|cap|vram"; "i"))
    | {($p | map(tostring) | join(".")): getpath($p)}] | add' "$OUT/show-$name.json" 2>/dev/null)"
  # what it ran lately: its TensorD log's run lines over the last 30 min
  cozy machine logs "$name" --since 30m -n 0 > "$OUT/log-$name.txt" 2>&1
  while IFS= read -r line; do say "  log: $line"; done < <(grep -iE "run|fail|error|oom|out of memory|cap|limit" "$OUT/log-$name.txt" | tail -40)
done
