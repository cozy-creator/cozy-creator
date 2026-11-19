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
#   after     a cozy-runtime commit: the bench runs again with its `files` laid over the pod's Runtime (the "after" arm)
#   files     those files, from src/cozy_runtime (default: cr-348's four)
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
quote 1800
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
T=60 pod "cat > /tmp/bench.py" < "$HERE/fixtures/exchange-bench/bench.py" || fail "bench.py"
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$RT" --tensorfs "$TF" -y || fail "the update"
step install package install "$SOURCE/ltx-2.5" --on "$NAME" || fail "the install"
py=$(T=60 pod "ls -t /var/lib/cozy/tensord/generations/*/env/bin/python | head -1" < /dev/null 2> /dev/null | tail -1)
[[ $py == /var/lib/cozy/* ]] || fail "no executor environment: $py"
T=60 pod "nvidia-smi topo -m; nvidia-smi --query-gpu=name,pcie.link.gen.max,pcie.link.width.max --format=csv" < /dev/null > "$OUT/topo.txt" 2>&1
# bench <arm> [PYTHONPATH]: one run of the fixture, its lines said.
bench() {
  T=1500 pod "PYTHONPATH=${2:-} $py /tmp/bench.py" < /dev/null > "$OUT/$1.ndjson" 2> "$OUT/$1.err"
  local code=$?
  grep '^{' "$OUT/$1.ndjson" | while read -r line; do say "$1 $line"; done
  [ "$code" = 0 ] && grep -q '"what": "meet"' "$OUT/$1.ndjson" || fail "$1: the bench ended $code: $(tail -c 1500 "$OUT/$1.err")"
}
bench before
AFTER=$(param after)
if [ -n "$AFTER" ]; then
  [[ $AFTER =~ ^[0-9a-f]{40}$ ]] || fail "after is not a commit: $AFTER"
  FILES=$(param files 'internal/parallel/copy_engine.py internal/parallel/ulysses.py internal/parallel/links.py internal/attention_ulysses.py')
  site=$(T=60 pod "$py -c 'import cozy_runtime, os; print(os.path.dirname(os.path.dirname(cozy_runtime.__file__)))'" < /dev/null 2> /dev/null | tail -1)
  [[ $site == /* ]] || fail "no site of the pod's Runtime: $site"
  T=120 pod "rm -rf /tmp/after && mkdir -p /tmp/after && cp -r $site/cozy_runtime /tmp/after/" < /dev/null || fail "the overlay"
  for f in $FILES; do
    curl -fsSL --retry 5 "https://raw.githubusercontent.com/cozy-creator/cozy-runtime/$AFTER/src/cozy_runtime/$f" -o "$OUT/after.py" || fail "fetch $f"
    T=60 pod "cat > /tmp/after/cozy_runtime/$f" < "$OUT/after.py" || fail "copy $f"
  done
  say "after: cozy-runtime ${AFTER:0:9}'s $FILES over the pod's Runtime"
  bench after /tmp/after
fi
say "PASS"
