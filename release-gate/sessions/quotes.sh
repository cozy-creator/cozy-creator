# quotes: the Hub's price now for each of params.skus (["rtx-4090", "rtx-3090:4", ...]) at the dispatch's disk, nothing
# rented: lib.sh `quote` for each in turn, a SKU no provider has said as such and the rest still quoted. Always $0.
. "$(dirname "$0")/lib.sh"
RENT=false
mapfile -t S < <(param skus '[]' | jq -r '.[]')
[ ${#S[@]} -gt 0 ] || fail "params.skus"
for s in "${S[@]}"; do ( quote 60 "$s" ) || say "no quote for $s"; done
say "quotes done"
