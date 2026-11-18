# bundle: several sessions on one rental of $SKU, one after another (`on_this`): one boot, and what one downloads the next finds.
# Each runs as it would alone, its files in $OUT/<session>; the bundle fails when one does. params:
#   sessions  [{"session": <name>, "params": {…}}, …] in order (required)
#   seconds   what the cap must buy for all of them (default 3600)
#   gpus      the rental's GPUs (1); rented with SSH, which ab.sh's derived state reads
. "$(dirname "$0")/lib.sh"
LIST=$(param sessions '[]')
[ "$(jq 'length' <<<"$LIST")" -gt 0 ] || fail "sessions: a list of {session, params}"
while read -r s; do [ -f "$(dirname "$0")/$s.sh" ] || fail "no session $s"; done < <(jq -r '.[].session' <<<"$LIST")
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
G=$(param gpus 1); [[ $G =~ ^[1-8]$ ]] || fail "gpus $G"
[ -f ~/.ssh/id_ed25519.pub ] || { mkdir -p ~/.ssh && ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519; } || fail "no SSH key"
RENT_ARGS=--ssh; [ "$G" = 1 ] || RENT_ARGS="--ssh --gpus $G"
trap 'code=$?; claim; (exit "$code"); finish' EXIT
quote "$(param seconds 3600)"
GUARD_ALSO=own guard
rent
for i in $(seq 0 $(($(jq 'length' <<<"$LIST") - 1))); do
  on_this "$(jq -r ".[$i].session" <<<"$LIST")" "$(jq -c ".[$i].params // {}" <<<"$LIST")"
done
exit "$RC"
