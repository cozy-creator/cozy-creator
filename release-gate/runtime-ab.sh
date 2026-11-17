# runtime-ab.sh, a --cells hook (`-f cells=release-gate/runtime-ab.sh -f runtime_wheel=…`): the varena change matrix (Paul
# 11-06: no row more than 5% worse) for a Runtime candidate against the Runtime the pod came with, on the same pod, as
# tensorfs-ab.sh does for TensorFS. Each model runs alone on the card (a restart before it), cold then warm, on the release
# (<cell>-released) and on the candidate (<cell>), A B B A (the second pass <row>-b), each arm from a cold pod: SDXL, Klein and Qwen-Image 4 MP with their prompt LMs on, H3 Turbo
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
AB_PASS=0  # the second pass of a row (A B B A) takes other seeds
# ab_on <runtime> <version it reports> <GiB|off>: a restart onto that Runtime under that cap, as `machine show` says.
ab_on() {
  local want=0 show said lim i line
  # An arm begins cold: what the last arm left on the pod (LoRA bakes, memos, peaks, kernels, page cache) goes before the
  # restart, and the census after it must find none (machines.sh `pod_derived`; 44006261838 compared a cold arm with a warm).
  [ -n "$(pod_derived "$CLI" "$M" reset)" ] || { log "runtime-ab: no derived-state reset on $M (an --ssh rental)"; return 1; }
  [ "$3" = off ] || want=$(($3 << 30))
  "$CLI" --json machine update "$M" --runtime "$1" --gpu-memory-limit "$([ "$3" = off ] && echo off || echo "${3}GiB")" -y \
    > ab-update.json 2> ab-update.log || { log "runtime-ab: update to $2 at $3: $(tail -c 300 ab-update.log | tr '\n' ' ')"; return 1; }
  for i in $(seq 36); do
    show=$("$CLI" --json machine show "$M" 2> /dev/null)
    said=$(jq -r '.status.runtime // ""' <<< "$show")
    lim=$(jq -r '[.status.gpus[]? | .memory_limit_bytes // 0 | tostring] | unique | join(",")' <<< "$show")
    [ "$said" = "$2" ] && [ "$lim" = "$want" ] && {
      line=$(pod_derived "$CLI" "$M" census); printf '%s\t%s\n' "$*" "$line" >> ab-derived.tsv
      pod_derived_zero "$line" && return 0
      log "runtime-ab: $* starts with derived state it did not make: ${line:-no census}"; return 1; }
    sleep 5
  done
  log "runtime-ab: machine show says Runtime ${said:-?} at ${lim:-?} bytes, not $2 at $want"
  return 1
}
# ab_model <model> <cell prefix> <suffix>: its cold and warm run; each its own prompt and seed, the same in both arms.
ab_model() {
  local m=$1 n=$2 sfx=$3 i v s
  local -a when=("at dusk" "in light rain" "under clear skies")
  # SDXL runs once more after a pause, when idle work has had its time: a graph captured idle serves only that run (as in
  # tensorfs-ab.sh; 46744/46748 had graphs on one arm's warm run and not the other's).
  for i in 0 1 2; do
    [ "$i" != 2 ] || [ "$m" = sdxl ] || continue
    [ "$i" != 2 ] || sleep 30
    v=${when[$i]}
    case $m in
      sdxl) s=$((R + 700 + i + 100 * AB_PASS))
        cell "$n-$i$sfx" "$(pkg sdxl generate)" prompt="A brass astrolabe on a sailor's chart table $v, warm lamplight" \
          megapixels:=1 steps:=20 guidance:=7 seed:=$s ;;
      klein) s=$((R + 710 + i + 100 * AB_PASS))
        cell "$n-$i$sfx" "$(pkg flux.3-klein generate_turbo)" prompt="A ceramic teapot on a windowsill $v, studio photo" \
          prompt_upsampling:=true seed:=$s ;;
      qwen4mp) s=$((R + 720 + i + 100 * AB_PASS))
        cell "$n-$i$sfx" "$(pkg qwen-image-2 generate)" prompt="A vintage tram crossing a cobbled square $v, fine architectural detail" \
          megapixels:=4 prompt_upsampling:=true seed:=$s ;;
      h3) s=$((R + 730 + i + 100 * AB_PASS))
        cell "$n-$i$sfx" "$(pkg minimax-h3 generate)" \
          "beats:=[{\"description\":\"[Shot 1]\\nA paper boat drifts down a rain-filled gutter past glowing shop windows $v. The camera follows at street level as drops ripple the water.\",\"duration\":15}]" \
          soundscape="Rain on pavement, distant traffic." turbo:=true seed:=$s ;;
      ideogram) s=$((R + 740 + i + 100 * AB_PASS))
        cell "$n-$i$sfx" "$(pkg ideogram-4 generate)" prompt="A bakery storefront $v with a painted sign that reads FRESH BREAD, photograph" \
          megapixels:=1 aspect_ratio=1:1 prompt_upsampling:=true seed:=$s ;;
      ph3) s=$((R + 750 + i + 100 * AB_PASS))
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
# Each row runs A B B A, every arm from a cold pod (`ab_on`), the second pass on other seeds (a repeat would be the call
# memo's): the order and the pod's drift show, and each pair is the same pass (analyze.py pairs <cell>-released with <cell>).
for lvl in off 8 6; do
  for m in h3 qwen4mp sdxl klein ideogram ph3; do
    [ "$m" != ideogram ] && [ "$m" != ph3 ] || [ "$lvl" = off ] || continue
    [ "$m" != h3 ] || [ "$lvl" != 6 ] || continue  # H3 15 s at 6 GiB fails on both arms (var-015, var-016)
    [[ " ${AB_DONE:-} " != *" $m-$lvl "* ]] || continue  # rows a pod already ran (a pod lost mid-matrix)
    AB_PASS=0 b=''; ab_arm "${AB_BASE_ON:-$AB_RELEASED}" "$AB_RELEASED" "$lvl" "$m" "ab-$m-$lvl$b" -released; ab_arm "$RWF" "$AB_CANDIDATE" "$lvl" "$m" "ab-$m-$lvl$b" ''
    AB_PASS=1 b=-b; ab_arm "$RWF" "$AB_CANDIDATE" "$lvl" "$m" "ab-$m-$lvl$b" ''; ab_arm "${AB_BASE_ON:-$AB_RELEASED}" "$AB_RELEASED" "$lvl" "$m" "ab-$m-$lvl$b" -released
  done
done
AB_PASS=0
