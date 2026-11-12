# Suite `slow` (cl-913): a run is never silently slow, on a real GPU machine. Each check is an `expect` row.
# - slow-refuses-pure: `cozy machine update` refuses a GPU machine the pure cozy-runtime wheel and a Runtime source checkout
#   (`runtime_without_kernels`), naming the dev-build that makes its wheel with kernels. Escaped as rabou's 46668/46678: GEMMs on
#   comfy-kitchen, FlashAttention-3's JIT failed.
# - slow-kernel-less: a Runtime whose native kernels are gone (moved aside over SSH) says so on every run on its GPUs
#   (`runtime_without_kernels`), and `machine show` says it; back in place, no run says it.
# - slow-hot: the machine on a hot host's recorded GPU readings (e2e/cli/slow/hot-host.jsonl, rabou's GPU 0 at 89 °C): a slow
#   still past its bar warns `gpu_throttled` and `slow_run` naming the heat as it runs, and `machine show` marks the machine.
S=$OUT/slow; mkdir -p "$S"
[ -z "$ONLY" ] || [[ $ONLY == *,slow* ]] || return 0
TOOL=$HERE/../e2e/cli/project/tool.py
codes() { jq -r '[.warnings[]?.code] | join(",")' "$OUT/$1.json" 2>/dev/null; }
restart() { "$CLI" machine update "$M" --gpu-memory-limit off -y > "$S/restart-$1.log" 2>&1; }

# Refused: the pure wheel of the Runtime release users get, and a source checkout.
rt=$("$CLI" --json machine show "$M" 2>/dev/null | jq -r '.status.runtime // empty' | sed 's/+.*//')
pip download -q --no-deps --only-binary=:all: --platform any --python-version 3.12 -d "$S/pure" "cozy-runtime==$rt" > "$S/pure.log" 2>&1 || true
pure=$(ls "$S"/pure/cozy_runtime-*-none-any.whl 2>/dev/null | head -1)
git clone -q --depth 1 https://github.com/cozy-creator/cozy-runtime "$S/checkout" > "$S/checkout.log" 2>&1 || true
bad=()
for what in "${pure:-}" "$S/checkout"; do
  [ -e "$what" ] || { bad+=("no ${what:-pure wheel of $rt}"); continue; }
  "$CLI" --json machine update "$M" --runtime "$what" -y > "$S/refused.json" 2>&1 && bad+=("$(basename "$what") was taken")
  jq -e '.error.code == "runtime_without_kernels" and (.error.next | length) == 2 and (.error.next[0] | test("dev-build"))' "$S/refused.json" > /dev/null 2>&1 \
    || bad+=("$(basename "$what"): $(head -c 300 "$S/refused.json")")
done
expect slow-refuses-pure "$([ ${#bad[@]} -eq 0 ] && echo PASS || echo FAIL)" "${bad[*]:-the pure wheel of $rt and a source checkout refused runtime_without_kernels, naming the dev-build}"

# Kernel-less: every native kernel module of the machine's Runtimes moved aside, executors started again.
"$CLI" machine ssh "$M" -- "find / -xdev -path '*cozy_runtime/_kernels/_*.so' -not -path '*/proc/*' 2>/dev/null | while read -r f; do mv \"\$f\" \"\$f.cl913\"; echo \"\$f\"; done" \
  < /dev/null > "$S/moved.txt" 2>&1
restart off
bad=() said=()
for i in 1 2; do
  run_cell "$OUT" "slow-kernel-less-$i" "$(pkg sdxl generate)" --on "$M" megapixels:=1.0 steps:=8 prompt="$OBJECT, kernel-less $i" seed:=$((R + 913 + i)) > /dev/null 2>&1 || true
  c=$(codes "slow-kernel-less-$i"); said+=("run $(cat "slow-kernel-less-$i.number" 2>/dev/null) $(run_get "$OUT" "slow-kernel-less-$i" .status) [$c]")
  [[ ,$c, == *,runtime_without_kernels,* ]] || bad+=("run $i said nothing")
done
shown=$("$CLI" --json machine show "$M" 2>/dev/null | jq -r '.status.runtime_kernels // empty')
[ -n "$shown" ] || bad+=("machine show says nothing")
"$CLI" machine ssh "$M" -- "while read -r f; do mv \"\$f.cl913\" \"\$f\"; done" < "$S/moved.txt" > "$S/restored.txt" 2>&1
restart on
run_cell "$OUT" slow-kernels-back "$(pkg sdxl generate)" --on "$M" megapixels:=1.0 steps:=8 prompt="$OBJECT, kernels back" seed:=$((R + 916)) > /dev/null 2>&1 || true
[[ ,$(codes slow-kernels-back), != *,runtime_without_kernels,* ]] || bad+=("with its kernels back it still says so")
[ -s "$S/moved.txt" ] || bad+=("no kernel module found to move")
expect slow-kernel-less "$([ ${#bad[@]} -eq 0 ] && echo PASS || echo FAIL)" "${bad[*]:-each run on the kernel-less Runtime said so, machine show too (${shown:0:120}), none once back}; ${said[*]}"

# Hot: three short stills its bar, then the machine on the hot host's readings and a 45 s still.
bad=()
for i in 1 2 3; do run_cell "$OUT" "slow-bar-$i" "$TOOL/slow" --on "$M" 'seconds:=1' "instructions=bar $i" > /dev/null 2>&1 || bad+=("bar $i"); done
"$CLI" machine ssh "$M" -- 'cat > /tmp/hot-host.jsonl' < "$HERE/../e2e/cli/slow/hot-host.jsonl" > /dev/null 2>&1
"$CLI" machine ssh "$M" -- "mkdir -p ~/.cozy && printf '\n[machine]\ngpu_samples = \"/tmp/hot-host.jsonl\"\n' >> ~/.cozy/config.toml" < /dev/null > /dev/null 2>&1
restart hot
run_cell "$OUT" slow-hot "$TOOL/slow" --on "$M" 'seconds:=45' 'instructions=hot' > /dev/null 2>&1 || bad+=("the hot run did not complete")
c=$(codes slow-hot) said=$(jq -r '.warnings[]? | select(.code == "slow_run") | .message' "$OUT/slow-hot.json" 2>/dev/null)
[[ ,$c, == *,gpu_throttled,* && ,$c, == *,slow_run,* ]] || bad+=("warned [$c]")
grep -q 'throttled for heat' <<< "$said" || bad+=("slow_run names no heat: $said")
heat=$("$CLI" --json machine show "$M" 2>/dev/null | jq -c '[.status.heat[]? | {gpu, share}]')
[[ $heat == *'"gpu":0'* ]] || bad+=("machine show marks no hot GPU: $heat")
"$CLI" machine ssh "$M" -- "sed -i '/gpu_samples = /d' ~/.cozy/config.toml" < /dev/null > /dev/null 2>&1
restart cool
expect slow-hot "$([ ${#bad[@]} -eq 0 ] && echo PASS || echo FAIL)" "${bad[*]:-run $(cat slow-hot.number 2>/dev/null) warned gpu_throttled and slow_run: $said; machine show $heat}"
