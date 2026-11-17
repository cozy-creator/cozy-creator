# io-fault (cr-318 step 2): a reader failing mid-run while another run is in flight leaves no other run's bytes wrong.
# cr-318, the coordinator 11-17: "Step 2 exists to keep one run's failure from corrupting another's slots, and that corruption
# would be silent, so it doesn't land without the deterministic proof."
# On one rental: an SDXL install from the packages checkout makes an executor environment (its Python and torch); per arm,
# that arm's TensorFS wheel is unpacked beside it and fixtures/io-fault/fault.py runs with it as root: two threads read 64
# tensors of 4 MiB each to the GPU at once, the private hook failing one staged piece a round. Every read that did not fail
# must hold its bytes, each failed one again after it, and at least one failed read must overlap the other thread's. A
# package can't run this: the package contract refuses `import tensorfs` (`internal_import`).
# params:
#   tree     the packages_refs checkout whose sdxl makes the environment (required)
#   runtime  the pod's Runtime (a cozy-runtime run id or https wheel)
#   arms     [{"name": "engine", "tensorfs": "<artifact:<run>/<name> or https wheel>", "judged": true}, …]
#   rounds   faults per arm (default 8)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
TREE=$(param tree '') ARMS=$(param arms '[]') ROUNDS=$(param rounds 8)
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/sdxl/pyproject.toml" ] || fail "dispatch with -f packages_refs=<name>=<the packages RC> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
mapfile -t names < <(jq -r '.[].name' <<<"$ARMS")
[ "${#names[@]}" -gt 0 ] && [[ $ROUNDS =~ ^[0-9]+$ ]] || fail "arms, rounds $ROUNDS"
RT=$(wheel_from "$OUT/wheels/runtime" "$(param runtime)") && [ -n "$RT" ] || fail "no Runtime wheel"
declare -A TF=()
for a in "${names[@]}"; do
  [[ $a =~ ^[a-z0-9-]+$ ]] || fail "arm name $a"
  TF[$a]=$(wheel_from "$OUT/wheels/$a" "$(jq -r --arg a "$a" '.[] | select(.name == $a) | .tensorfs' <<<"$ARMS")")
  [ -n "${TF[$a]}" ] || fail "arm $a: no TensorFS wheel"
  say "arm $a: ${TF[$a]##*/}"
done
kernels "$SOURCE/sdxl"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote 1800
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
T=60 pod "cat > /tmp/fault.py" < "$HERE/fixtures/io-fault/fault.py" || fail "fault.py"
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$RT" --tensorfs "${TF[${names[0]}]}" -y || fail "the update"
step install package install "$SOURCE/sdxl" --on "$NAME" || fail "the install"
py=$(T=60 pod "ls -t /var/lib/cozy/tensord/generations/*/env/bin/python | head -1" < /dev/null 2> /dev/null | tail -1)
[[ $py == /var/lib/cozy/* ]] || fail "no executor environment: $py"
for a in "${names[@]}"; do
  T=300 pod "rm -rf /tmp/tf-$a && mkdir -p /tmp/tf-$a && cat > /tmp/tf-$a.whl && $py -m zipfile -e /tmp/tf-$a.whl /tmp/tf-$a" < "${TF[$a]}" ||
    fail "$a: the wheel to the pod"
  T=900 pod "PYTHONPATH=/tmp/tf-$a $py /tmp/fault.py 0 $ROUNDS" < /dev/null > "$OUT/$a.json" 2> "$OUT/$a.err"
  judged=$(jq -r --arg a "$a" '.[] | select(.name == $a) | .judged // false' <<<"$ARMS")
  if jq -e '.pass == true' "$OUT/$a.json" > /dev/null 2>&1; then
    say "PASS $a: $(jq -c '{tensorfs, from, overlapped_rounds, rounds: [.rounds[] | {failed: (.failed | length), overlapped: (.overlapped | length), wrong: (.wrong | length), reread_ok, gb_s}]}' "$OUT/$a.json")"
  else
    say "FAIL $a$([ "$judged" = true ] || echo ' (not judged)'): $(head -c 600 "$OUT/$a.json") $(tail -c 400 "$OUT/$a.err")"
    [ "$judged" != true ] || RC=1
  fi
done
exit "$RC"
