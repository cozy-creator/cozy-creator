# bundle: several sessions on one rental of $SKU, one after another (`on_this`): one boot, and what one downloads the next finds.
# Each runs as it would alone, its files in $OUT/<session>; the bundle fails when one does. params:
#   sessions  [{"session": <name>, "params": {…}}, …] in order (required)
#   seconds   what the cap must buy for all of them (default 3600)
. "$(dirname "$0")/lib.sh"
LIST=$(param sessions '[]')
[ "$(jq 'length' <<<"$LIST")" -gt 0 ] || fail "sessions: a list of {session, params}"
while read -r s; do [ -f "$(dirname "$0")/$s.sh" ] || fail "no session $s"; done < <(jq -r '.[].session' <<<"$LIST")
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
trap 'code=$?; claim; (exit "$code"); finish' EXIT
quote "$(param seconds 3600)"
GUARD_ALSO=own guard
rent
for i in $(seq 0 $(($(jq 'length' <<<"$LIST") - 1))); do
  on_this "$(jq -r ".[$i].session" <<<"$LIST")" "$(jq -c ".[$i].params // {}" <<<"$LIST")"
done
exit "$RC"
