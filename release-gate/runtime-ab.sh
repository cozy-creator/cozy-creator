# runtime-ab.sh, a --cells hook (`-f cells=release-gate/runtime-ab.sh -f runtime_wheel=…`): the varena change matrix (Paul
# 11-06: no row more than 5% worse) for a Runtime candidate against the Runtime the pod came with, on the same pod, as
# tensorfs-ab.sh does for TensorFS. Each model runs alone on the card (a restart before it), cold then warm, on the release
# (<cell>-released) and then on the candidate (<cell>): SDXL, Klein and Qwen-Image 4 MP with their prompt LMs on, H3 Turbo
# 15 s, uncapped and under the development cap at 8 and 6 GiB (H3 not at 6: var-015); Ideogram 4 and P-H3-Image's edit
# with its upsampler, uncapped. analyze.py notes each pair's execution, steady step and output.
AB_RELEASED=$(awk '$1 == "runtime" {print $2}' machine-fresh.txt) AB_CANDIDATE=$rt AB_BASE_ON=
# AB_BASE=<wheel> AB_BASE_VERSION=<the version it reports>: that Runtime is the base arm instead (an RC head, to attribute a
# change to its own commits).
if [ -n "${AB_BASE:-}" ]; then AB_BASE_ON=$AB_BASE AB_RELEASED=${AB_BASE_VERSION:?AB_BASE needs AB_BASE_VERSION}; fi
if [ -z "$RWF" ] || [ -z "$AB_RELEASED" ] || [ "$AB_RELEASED" = "$AB_CANDIDATE" ]; then
  expect runtime-ab FAIL "needs a runtime_wheel other than the pod's own Runtime (${AB_RELEASED:-none}; candidate ${AB_CANDIDATE:-none})"
  return 0
fi
ONLY=  # the suites' --only chose their cells, not these
# ab_on <runtime> <version it reports> <GiB|off>: a restart onto that Runtime under that cap, as `machine show` says.
ab_on() {
  local want=0 show said lim i
  [ "$3" = off ] || want=$(($3 << 30))
  "$CLI" --json machine update "$M" --runtime "$1" --gpu-memory-limit "$([ "$3" = off ] && echo off || echo "${3}GiB")" -y \
    > ab-update.json 2> ab-update.log || { log "runtime-ab: update to $2 at $3: $(tail -c 300 ab-update.log | tr '\n' ' ')"; return 1; }
  for i in $(seq 36); do
    show=$("$CLI" --json machine show "$M" 2> /dev/null)
    said=$(jq -r '.status.runtime // ""' <<< "$show")
    lim=$(jq -r '[.status.gpus[]? | .memory_limit_bytes // 0 | tostring] | unique | join(",")' <<< "$show")
    [ "$said" = "$2" ] && [ "$lim" = "$want" ] && return 0
    sleep 5
  done
  log "runtime-ab: machine show says Runtime ${said:-?} at ${lim:-?} bytes, not $2 at $want"
  return 1
}
# ab_model <model> <cell prefix> <suffix>: its cold and warm run; each its own prompt and seed, the same in both arms.
ab_model() {
  local m=$1 n=$2 sfx=$3 i v s
  local -a when=("at dusk" "in light rain")
  for i in 0 1; do
    v=${when[$i]}
    case $m in
      sdxl) s=$((R + 700 + i))
        cell "$n-$i$sfx" "$(pkg sdxl generate)" prompt="A brass astrolabe on a sailor's chart table $v, warm lamplight" \
          megapixels:=1 steps:=20 guidance:=7 seed:=$s ;;
      klein) s=$((R + 710 + i))
        cell "$n-$i$sfx" "$(pkg flux.3 generate_turbo)" prompt="A ceramic teapot on a windowsill $v, studio photo" \
          prompt_upsampling:=true seed:=$s ;;
      qwen4mp) s=$((R + 720 + i))
        cell "$n-$i$sfx" "$(pkg qwen-image-2 generate)" prompt="A vintage tram crossing a cobbled square $v, fine architectural detail" \
          megapixels:=4 prompt_upsampling:=true seed:=$s ;;
      h3) s=$((R + 730 + i))
        cell "$n-$i$sfx" "$(pkg minimax-h3 generate)" \
          "beats:=[{\"description\":\"[Shot 1]\\nA paper boat drifts down a rain-filled gutter past glowing shop windows $v. The camera follows at street level as drops ripple the water.\",\"duration\":15}]" \
          soundscape="Rain on pavement, distant traffic." turbo:=true seed:=$s ;;
      ideogram) s=$((R + 740 + i))
        cell "$n-$i$sfx" "$(pkg ideogram-4 generate)" prompt="A bakery storefront $v with a painted sign that reads FRESH BREAD, photograph" \
          megapixels:=1 aspect_ratio=1:1 prompt_upsampling:=true seed:=$s ;;
      ph3) s=$((R + 750 + i))
        cell "$n-$i$sfx" "$(pkg p-h3-image generate)" references:="[{\"name\":\"photo\",\"image\":\"$REF\"}]" \
          instructions="<photo> $v, the sky turned deep orange, everything else kept" seed:=$s ;;
    esac
  done
}
# ab_arm <runtime> <version> <cap> <model> <prefix> <suffix>: a restart onto it, then the model alone.
ab_arm() {
  if ab_on "$1" "$2" "$3"; then ab_model "$4" "$5" "$6"; else expect "$5$6" FAIL "$2 did not go on at $3"; fi
}
log "runtime-ab: release $AB_RELEASED against candidate $AB_CANDIDATE"
# Which arm goes first alternates by level: the second one's cold run finds the weights in the page cache.
for lvl in off 8 6; do
  for m in h3 qwen4mp sdxl klein ideogram ph3; do
    [ "$m" != ideogram ] && [ "$m" != ph3 ] || [ "$lvl" = off ] || continue
    [ "$m" != h3 ] || [ "$lvl" != 6 ] || continue  # H3 15 s at 6 GiB fails on both arms (var-015, var-016)
    [[ " ${AB_DONE:-} " != *" $m-$lvl "* ]] || continue  # rows a pod already ran (a pod lost mid-matrix)
    [ "$lvl" = 8 ] || ab_arm "${AB_BASE_ON:-$AB_RELEASED}" "$AB_RELEASED" "$lvl" "$m" "ab-$m-$lvl" -released
    ab_arm "$RWF" "$AB_CANDIDATE" "$lvl" "$m" "ab-$m-$lvl" ''
    [ "$lvl" != 8 ] || ab_arm "${AB_BASE_ON:-$AB_RELEASED}" "$AB_RELEASED" "$lvl" "$m" "ab-$m-$lvl" -released
  done
done
