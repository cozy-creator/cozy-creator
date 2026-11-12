# The 0.25.0 RTX 5090 cold proof (var-016 reverted; coordinator 11-12), a --cells hook after suite bench's h3-15s-5090 (cold, warm):
# H3 15 s --lane int8 cold twice more on this pod's candidate set, a machine restart before each; then once cold on TensorFS
# 0.25.0, the candidate with var-016, as the A/B; then ComfyUI on this pod for the bench cell (comfyui.sh).
IFS=$'\t' read -r _ _ _ _ seed target args _ < <(awk -F'\t' '$1 == "h3-15s-5090"' "$HERE/suites/bench/cells.tsv")
t=$(pkg "${target%/*}" "${target##*/}")
ONLY=
# cold <name> <seed> [tensorfs]: a restart (onto that TensorFS), then the run.
cold() {
  retried "$CLI" machine update "$M" ${3:+--tensorfs "$3"} --gpu-memory-limit off -y < /dev/null > "$1-restart.log" 2>&1 \
    || log "$1: restart: $(tail -c 300 "$1-restart.log" | tr '\n' ' ')"
  local i
  for i in $(seq 36); do
    [ -z "${3:-}" ] || [ "$("$CLI" --json machine show "$M" 2> /dev/null < /dev/null | jq -r '.status.tensorfs // ""')" = "$3" ] && break
    sleep 5
  done
  log "$1 on $("$CLI" --json machine show "$M" 2> /dev/null < /dev/null | jq -r '"cozy \(.status.version) rt\(.status.runtime) tfs\(.status.tensorfs)"')"
  eval "cell \"\$1\" \"\$t\" $args seed:=$2"
}
cold h3-cold-rev-1 $((seed + 500001))
cold h3-cold-rev-2 $((seed + 500002))
cold h3-cold-tfs0250 $((seed + 500003)) 0.25.0
. "$HERE/suites/bench/comfyui.sh"
