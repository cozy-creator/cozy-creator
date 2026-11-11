# tensorfs-ab.sh, a --cells hook (`-f cells=release-gate/tensorfs-ab.sh -f tensorfs_wheel=…`): the varena matrix (Paul 11-06:
# no varena change lands without it, no row more than 5% worse), a TensorFS candidate against the release the pod came with,
# on the same pod. At each level (uncapped, then the development cap at 8 and 6 GiB) SDXL, Klein, Qwen-Image 4 MP and H3
# Turbo 15 s run cold then warm on the release (<cell>-released), then on the candidate (<cell>); uncapped, both again on new
# seeds (-released-again, -again). analyze.py notes each pair's execution, steady step and output.
AB_RELEASED=$(awk '$1 == "tensorfs" {print $2}' machine-fresh.txt) AB_CANDIDATE=$tfs
if [ -z "$TWF" ] || [ -z "$AB_RELEASED" ] || [ "$AB_RELEASED" = "$AB_CANDIDATE" ]; then
  expect tensorfs-ab FAIL "needs a tensorfs_wheel other than the pod's own TensorFS (${AB_RELEASED:-none}; candidate ${AB_CANDIDATE:-none})"
  return 0
fi
ONLY=  # the suites' --only chose their cells, not these
# ab_on <tensorfs> <version it reports> <GiB|off>: the machine on that TensorFS under that cap, as `machine show` says.
ab_on() {
  local want=0 show said lim i
  [ "$3" = off ] || want=$(($3 << 30))
  "$CLI" --json machine update "$M" --tensorfs "$1" --gpu-memory-limit "$([ "$3" = off ] && echo off || echo "${3}GiB")" -y \
    > ab-update.json 2> ab-update.log || { log "tensorfs-ab: update to $2 at $3: $(tail -c 300 ab-update.log | tr '\n' ' ')"; return 1; }
  for i in $(seq 36); do
    show=$("$CLI" --json machine show "$M" 2> /dev/null)
    said=$(jq -r '.status.tensorfs // ""' <<< "$show")
    lim=$(jq -r '[.status.gpus[]? | .memory_limit_bytes // 0 | tostring] | unique | join(",")' <<< "$show")
    [ "$said" = "$2" ] && [ "$lim" = "$want" ] && return 0
    sleep 5
  done
  log "tensorfs-ab: machine show says TensorFS ${said:-?} at ${lim:-?} bytes, not $2 at $want"
  return 1
}
# ab_round <suffix> <level> <round>: every model, cold then warm; each run its own prompt and seed, the same in both arms.
ab_round() {
  local sfx=$1 lvl=$2 r=$3 i v s
  local -a when=("at dusk" "in light rain")
  for i in 0 1; do
    v=${when[$i]} s=$((R + 700 + 100 * r + i))
    cell "ab-sdxl-$lvl-$i$sfx" "$(pkg sdxl generate)" \
      prompt="A brass astrolabe on a sailor's chart table $v, warm lamplight" megapixels:=1 steps:=20 guidance:=7 seed:=$s
  done
  for i in 0 1; do
    v=${when[$i]} s=$((R + 710 + 100 * r + i))
    cell "ab-klein-$lvl-$i$sfx" "$(pkg flux-3-klein generate_turbo)" \
      prompt="A ceramic teapot on a windowsill $v, studio photo" prompt_upsampling:=false seed:=$s
  done
  for i in 0 1; do
    v=${when[$i]} s=$((R + 720 + 100 * r + i))
    cell "ab-qwen4mp-$lvl-$i$sfx" "$(pkg qwen-image-2 generate)" \
      prompt="A vintage tram crossing a cobbled square $v, fine architectural detail" megapixels:=4 prompt_upsampling:=false seed:=$s
  done
  for i in 0 1; do
    v=${when[$i]} s=$((R + 730 + 100 * r + i))
    cell "ab-h3-$lvl-$i$sfx" "$(pkg minimax-h3 generate)" \
      "beats:=[{\"description\":\"[Shot 1]\\nA paper boat drifts down a rain-filled gutter past glowing shop windows $v. The camera follows at street level as drops ripple the water.\",\"duration\":15}]" \
      soundscape="Rain on pavement, distant traffic." turbo:=true seed:=$s
  done
}
log "tensorfs-ab: release $AB_RELEASED against candidate $AB_CANDIDATE"
for lvl in off 8 6; do
  rounds=0; [ "$lvl" != off ] || rounds=1
  for r in $(seq 0 $rounds); do
    again=; [ "$r" = 0 ] || again=-again
    if ab_on "$AB_RELEASED" "$AB_RELEASED" "$lvl"; then ab_round "-released$again" "$lvl" "$r"
    else expect "tensorfs-ab-$lvl$again" FAIL "the release did not go on at $lvl"; continue; fi
    if ab_on "$TWF" "$AB_CANDIDATE" "$lvl"; then ab_round "$again" "$lvl" "$r"
    else expect "tensorfs-ab-$lvl$again" FAIL "the candidate did not go back on at $lvl"; fi
  done
done
