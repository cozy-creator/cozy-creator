# exchange-bench (cr-348): what a 2-GPU group's crossings cost where its GPUs have no peer access, through the Runtime's own
# route. LTX-2.5 at 2 GPUs lost to 1 (gate 44411764104, 2x rtx-5090): its host-staged exchange showed 2.8 GB/s against the pair's
# measured 47.5. On one 2-GPU rental with SSH: an ltx-2.5 install from the packages checkout makes an executor environment, and
# fixtures/exchange-bench/bench.py runs with its Python as root: the links, the exchange and NCCL at 1-256 MiB, the Ulysses
# exchange and the package's cross-modal crossings at LTX's 1080p shapes, whether each returns before the GPU drains, and the
# per-call rendezvous. A package can't run this: the package contract refuses Runtime internals.
# params:
#   tree      the packages_refs checkout whose ltx-2.5 makes the environment (required)
#   runtime   the pod's Runtime (a cozy-runtime dev-build run id or https wheel, a literal + in its name)
#   tensorfs  the pod's TensorFS (artifact:<run>/<name> or https wheel)
#   provider  the rental's provider (a dev cozy only)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
TREE=$(param tree '')
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/ltx-2.5/pyproject.toml" ] || fail "dispatch with -f packages_refs=<name>=<the packages RC> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
[ -f "$HERE/fixtures/exchange-bench/bench.py" ] || fail "no fixtures/exchange-bench/bench.py"
RT=$(wheel_from "$OUT/wheels/runtime" "$(param runtime)") && [ -n "$RT" ] || fail "no Runtime wheel"
TF=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs)") && [ -n "$TF" ] || fail "no TensorFS wheel"
say "runtime ${RT##*/}, tensorfs ${TF##*/}"
kernels "$SOURCE/ltx-2.5"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
PROVIDER=$(param provider)
RENT_ARGS="--ssh --gpus 2${PROVIDER:+ --provider $PROVIDER}"
quote 2400
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
T=60 pod "cat > /tmp/bench.py" < "$HERE/fixtures/exchange-bench/bench.py" || fail "bench.py"
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$RT" --tensorfs "$TF" -y || fail "the update"
step install package install "$SOURCE/ltx-2.5" --on "$NAME" || fail "the install"
py=$(T=60 pod "ls -t /var/lib/cozy/tensord/generations/*/env/bin/python | head -1" < /dev/null 2> /dev/null | tail -1)
[[ $py == /var/lib/cozy/* ]] || fail "no executor environment: $py"
T=60 pod "nvidia-smi topo -m; nvidia-smi --query-gpu=name,pcie.link.gen.max,pcie.link.width.max --format=csv" < /dev/null > "$OUT/topo.txt" 2>&1
T=1500 pod "$py /tmp/bench.py" < /dev/null > "$OUT/bench.ndjson" 2> "$OUT/bench.err"
code=$?
grep '^{' "$OUT/bench.ndjson" | while read -r line; do say "$line"; done
[ "$code" = 0 ] && grep -q '"what": "meet"' "$OUT/bench.ndjson" || fail "the bench ended $code: $(tail -c 1500 "$OUT/bench.err")"
say "PASS"
