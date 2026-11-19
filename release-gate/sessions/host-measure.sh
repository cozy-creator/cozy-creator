# host-measure (var-030, PROOF ONLY): one rental, its host's memory bounds and rates beside one capped cell, for cr-336's
# low-VRAM decode design. After the cell: the bound TensorD enforces on the executor (each cgroup's memory.max and current
# on its path, MemTotal), the cell's host tier record, the pod's memory log, then on the executor's own Python: host copy
# and read bandwidth, pinned and pageable H2D, and a token's GEMV on the CPU (bf16, int8 dynamic, int4 packed) and on the
# card, each as GB/s of weights read.
# params: tree, target, args (a cell, as ab.sh's), cap, runtime, tensorfs (wheel sources as ab.sh's update), need_s
. "$(dirname "$0")/lib.sh"
TREE=$(param tree rc) TARGET=$(param target '') CAPV=$(param cap 6GiB) NEED=$(param need_s 3600)
RT=$(param runtime '') TF=$(param tensorfs '')
[ -n "$TARGET" ] && [ -d "$PACKAGES/$TREE" ] || fail "no target or no packages checkout $TREE"
mapfile -t ARGS < <(param args '[]' | jq -r '.[]')
RWF=$(wheel_from "$OUT/wheels/runtime" "$RT") && [ -n "$RWF" ] || fail "no runtime wheel from $RT"
TWF=$(wheel_from "$OUT/wheels/tensorfs" "$TF") && [ -n "$TWF" ] || fail "no tensorfs wheel from $TF"
kernels "$PACKAGES/$TREE/${TARGET%%/*}"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$NEED"
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$RWF" --tensorfs "$TWF" --gpu-memory-limit "$CAPV" -y ||
  fail "update: $(tail -c 300 "$OUT/update.txt")"
# pod <script> [seconds]: the pod's root shell, all its output.
pod() {
  timeout "${2:-120}" cozy machine ssh "$NAME" "echo $(base64 -w0 <<< "$1") | base64 -d | sh" < /dev/null 2> /dev/null |
    grep -v '^Warning: Permanently added'
}
say "host: $(pod 'echo "$(nproc) cores, $(grep -m1 "model name" /proc/cpuinfo | cut -d: -f2), MemTotal $(awk "/^MemTotal/ {print \$2 * 1024}" /proc/meminfo) B"' | tail -1)"
A=()
for a in "${ARGS[@]}"; do A+=("${a//\{ref\}/$REF}"); done
step cell run "$PACKAGES/$TREE/./$TARGET" "${A[@]}" --on "$NAME" || say "cell: $(tail -c 400 "$OUT/cell.txt")"
N=$(runno cell)
[ -z "$N" ] || (cd "$RUN_PROJECT" && cozy run show "$N" --json --full > "$OUT/cell.json" 2>&1)
jq -c '[.measurements.memory.stats.varena[0].stats | {baked_pages: (.baked_pages | del(.store_reads)), moved, weight_residency, streamed, gathered}]' \
  "$OUT/cell.json" > "$OUT/tier.json" 2>/dev/null && say "tier: $(cut -c1-1500 "$OUT/tier.json")"
# The bound on the executor's path: each cgroup from the executor's own up to the root.
pod 'p=$(pgrep -f cozy_runtime.internal.executor | head -1); [ -n "$p" ] || p=$(pgrep -x tensord | head -1); echo "pid $p"
c=$(awk -F: "\$1 == 0 {print \$3}" /proc/$p/cgroup); while :; do d=/sys/fs/cgroup$c
  echo "cgroup $c max $(cat $d/memory.max 2>/dev/null) current $(cat $d/memory.current 2>/dev/null) high $(cat $d/memory.high 2>/dev/null)"
  [ "$c" = / ] || [ -z "$c" ] && break; c=$(dirname "$c"); done; grep -E "^(MemTotal|MemAvailable|Cached|Unevictable|Mlocked):" /proc/meminfo' 60 > "$OUT/bounds.txt"
while read -r l; do say "bound: $l"; done < "$OUT/bounds.txt"
timeout 300 cozy machine logs "$NAME" memory -n 0 > "$OUT/memory.log" 2>&1
timeout 300 cozy machine logs "$NAME" -n 0 > "$OUT/tensord.log" 2>&1
say "memory log: $(wc -l < "$OUT/memory.log") lines; tier lines: $(grep -ciE 'tier|pages|host|headroom|short' "$OUT/memory.log")"
MEASURE='import os, time, torch
torch.set_num_threads(os.cpu_count())
def rate(f, nbytes, n=3):
    f(); best = 1e9
    for _ in range(n):
        t = time.perf_counter(); f(); best = min(best, time.perf_counter() - t)
    return nbytes / best / 1e9
x = torch.empty(4 << 30, dtype=torch.uint8).fill_(1); y = torch.empty_like(x)
print("host copy GB/s (read+write counted once)", round(rate(lambda: y.copy_(x), x.numel()), 1))
f = x.view(torch.float32)
print("host read GB/s (sum f32)", round(rate(lambda: f.sum(), x.numel()), 1))
del y, f
cuda = torch.cuda.is_available()
if cuda:
    p = torch.empty(1 << 30, dtype=torch.uint8, pin_memory=True); q = torch.empty(1 << 30, dtype=torch.uint8); d = torch.empty(1 << 30, dtype=torch.uint8, device="cuda")
    def h2d(src):
        d.copy_(src, non_blocking=True); torch.cuda.synchronize()
    print("H2D pinned GB/s", round(rate(lambda: h2d(p), 1 << 30), 1)); print("H2D pageable GB/s", round(rate(lambda: h2d(q), 1 << 30), 1))
    del p, q, d
for K, N in ((5120, 25600), (5120, 248320)):
    a = torch.randn(1, K, dtype=torch.bfloat16); w = torch.randn(N, K, dtype=torch.bfloat16)
    print(f"cpu bf16 gemv {K}x{N} GB/s", round(rate(lambda: torch.nn.functional.linear(a, w), w.numel() * 2), 1))
    try:
        wq = torch.randint(-127, 127, (N, K), dtype=torch.int8); s = torch.rand(N, dtype=torch.bfloat16)
        print(f"cpu int8 gemv (dequant per row, bf16) {K}x{N} GB/s", round(rate(lambda: torch.nn.functional.linear(a, wq.to(torch.bfloat16)) * s, wq.numel()), 1))
    except Exception as e:
        print("cpu int8 gemv failed", type(e).__name__, str(e)[:120])
    try:
        packed = torch.ops.aten._convert_weight_to_int4pack_for_cpu(torch.randint(0, 16, (N, K), dtype=torch.int32), 2)
        sz = torch.rand(K // 128, N, 2, dtype=torch.bfloat16)
        print(f"cpu int4 gemv (int4pack, group 128) {K}x{N} GB/s", round(rate(lambda: torch.ops.aten._weight_int4pack_mm_for_cpu(a, packed, 128, sz), N * K // 2), 1))
    except Exception as e:
        print("cpu int4 gemv failed", type(e).__name__, str(e)[:160])
    if cuda:
        ag, wg = a.cuda(), w.cuda()
        def g():
            torch.nn.functional.linear(ag, wg); torch.cuda.synchronize()
        print(f"gpu bf16 gemv {K}x{N} GB/s", round(rate(g, wg.numel() * 2), 1)); del ag, wg
    del a, w
'
PY=$(pod 'ls -t /var/lib/cozy/tensord/generations/*/env/bin/python 2>/dev/null | head -1' | tail -1)
say "python: $PY"
[ -n "$PY" ] || fail "no executor Python on the pod"
pod "$PY - <<'PYEOF'
$MEASURE
PYEOF" 1200 > "$OUT/measure.txt" 2>&1
while read -r l; do say "measure: $l"; done < "$OUT/measure.txt"
