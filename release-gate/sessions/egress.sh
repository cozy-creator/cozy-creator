# egress: a pod fetches nothing at the metadata address (xs-196 D). One CPU rental, no GPU work:
#   1. the pod's TensorD becomes the session's cozy (`cozy machine update --tensord`);
#   2. a model source at https://169.254.169.254 is refused as link-local, on the pod.
# No params.
. "$(dirname "$0")/lib.sh"
quote 900
rent

step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
say "pod TensorD $v"

if step metadata model download https://169.254.169.254/latest/meta-data/model.safetensors local/xs196-metadata --on "$NAME"; then
  fail "the pod fetched a model from the metadata address"
fi
grep -q link-local "$OUT/metadata.txt" || fail "the metadata source was not refused as link-local: $(tail -c 400 "$OUT/metadata.txt")"
say "PASS: the pod refused https://169.254.169.254 as link-local"
