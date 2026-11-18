# cpu-ab (cr-337/cr-339, proof only, never merged): one runner, its own `local` machine, two TensorD+TensorFS stacks on the
# session's Runtime, A B B A, e2e's cli-words call (Qwen3.5 0.8B bf16 on the CPU through qwen3/write) in each: the host's
# CPU, then each call's seconds, decode rate and weight reads. Never rents: it ends before `quote`, a $0 pre-flight.
# params: a_tensord, b_tensord (cozy-creator dev-build run ids), a_tensorfs, b_tensorfs (artifact:<run>/<name>), calls (2)
. "$(dirname "$0")/lib.sh"
CALLS=$(param calls 2)
[ -n "${RUNTIME_WHEEL:-}" ] || fail "no RUNTIME_WHEEL (dispatch from rc/<x>)"
[ -d "$PACKAGES/pk/qwen3" ] || fail "no packages/pk/qwen3 (packages_refs pk=<ref>)"
{
  lscpu | grep -E '^(Model name|CPU\(s\)|Thread\(s\) per core|Core\(s\) per socket|Socket\(s\)|L3 cache)'
  echo "flags: $(grep -m1 '^flags' /proc/cpuinfo | grep -oE '\b(avx2|avx512f|avx512_bf16|amx_bf16|amx_tile)\b' | sort -u | paste -sd' ')"
  echo "nproc: $(nproc)"
  echo "memory: $(free -g | awk '/^Mem/ {print $2 " GiB total, " $7 " GiB available"}')"
} | while read -r line; do say "host: $line"; done
cozy_of() {
  local d=$OUT/cozy-$1
  gh run download "$1" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$d" > /dev/null 2>&1 || return 1
  install -m 0755 "$d"/cozy-*/cozy-linux-x86_64 "$d/cozy" && echo "$d/cozy"
}
A_TD=$(cozy_of "$(param a_tensord)") || fail "a_tensord"
B_TD=$(cozy_of "$(param b_tensord)") || fail "b_tensord"
A_TFS=$(wheel_from "$OUT/wheels/a" "$(param a_tensorfs)") && [ -n "$A_TFS" ] || fail "a_tensorfs"
B_TFS=$(wheel_from "$OUT/wheels/b" "$(param b_tensorfs)") && [ -n "$B_TFS" ] || fail "b_tensorfs"
say "A: TensorD $("$A_TD" --version) · ${A_TFS##*/}"
say "B: TensorD $("$B_TD" --version) · ${B_TFS##*/}"
say "Runtime: ${RUNTIME_WHEEL##*/}"
printf 'call\tarm\tseconds\tdecode\treads\n' > "$OUT/calls.tsv"
n=0
for arm in a b b a; do
  if [ "$arm" = a ]; then td=$A_TD tfs=$A_TFS; else td=$B_TD tfs=$B_TFS; fi
  step "update-$arm-$n" machine update local --tensord "$td" --tensorfs "$tfs" --runtime "$RUNTIME_WHEEL" -y \
    || fail "local refused arm $arm: $(tail -c 400 "$OUT/update-$arm-$n.txt")"
  say "arm $arm: local runs $(cozy --json machine show local | jq -r '"TensorD \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs)"')"
  for i in $(seq 0 "$CALLS"); do  # call 0 warms the arm (the executor, and the weights on the first arm): not compared
    n=$((n + 1)); k=$arm-$n-$i
    t0=$(date +%s.%N)
    step "$k" run "$PACKAGES/pk/qwen3/write" 'instructions=Write eight sentences about a lighthouse at night.' \
      --model model=huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0 max_tokens:=240 "seed:=$((7 + n))" temperature:=0 --on local \
      || fail "$k: $(tail -c 400 "$OUT/$k.txt")"
    s=$(awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN { printf "%.1f", b - a }')
    N=$(runno "$k"); (cd "$RUN_PROJECT" && cozy run watch "$N" --once --json --full > "$OUT/$k.json" 2> /dev/null)
    decode=$(jq -r '[.measurements.residency.notes // {} | to_entries[] | .value] | join(" | ")' "$OUT/$k.json" 2> /dev/null | cut -c1-300)
    reads=$(jq -r '[.. | objects | select(has("read_bytes")) | .read_bytes] | add // 0' "$OUT/$k.json" 2> /dev/null)
    printf '%s\t%s\t%s\t%s\t%s\n' "$k" "$arm" "$s" "$decode" "$reads" >> "$OUT/calls.tsv"
    say "$k (arm $arm$([ "$i" = 0 ] && echo ', warm-up')): ${s} s; reads ${reads} B; ${decode}"
  done
done
column -t -s $'\t' "$OUT/calls.tsv" | while read -r line; do say "$line"; done
say "RESULT PASSED"
exit 0
