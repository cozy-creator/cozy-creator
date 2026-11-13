# varena-ab.sh, a --cells hook: tensorfs-ab.sh's varena matrix for a change to TensorD and TensorFS together (a varena ledger
# layout change, xs-189: a TensorD and an executor of different layouts don't meet), on one pod. An arm is a cozy binary and
# a TensorFS wheel from one build, put on together by one `machine update`: the base (<cell>-released), e.g. the RC head's dev
# build, and the candidate (<cell>). The same cells, prompts, seeds, caps and order as tensorfs-ab.sh. By hand, on a pod:
#   AB_BASE_TENSORD=<cozy> AB_BASE_TENSORFS=<whl> AB_TENSORD=<cozy> AB_TENSORFS=<whl> \
#     gate.sh <SKU> --on <pod> --suite inputs --only varena-ab --establish --cells release-gate/varena-ab.sh
# AB_SKIP="ab-h3-off …" leaves out rows a run already has.
ONLY=  # the suites' --only chose their cells, not these
ab_version() { case $1 in *.whl) basename "$1" | cut -d- -f2 ;; *) "$1" --version | awk '{print $2}' ;; esac; }
for f in "${AB_BASE_TENSORD:-}" "${AB_BASE_TENSORFS:-}" "${AB_TENSORD:-}" "${AB_TENSORFS:-}"; do
  [ -f "$f" ] || { expect varena-ab FAIL "needs AB_BASE_TENSORD, AB_BASE_TENSORFS, AB_TENSORD and AB_TENSORFS files (${f:-unset})"; return 0; }
done
chmod +x "$AB_BASE_TENSORD" "$AB_TENSORD"
AB_BASE="$AB_BASE_TENSORD $AB_BASE_TENSORFS $(ab_version "$AB_BASE_TENSORD") $(ab_version "$AB_BASE_TENSORFS")"
AB_CAND="$AB_TENSORD $AB_TENSORFS $(ab_version "$AB_TENSORD") $(ab_version "$AB_TENSORFS")"
[ "$(cut -d' ' -f3- <<< "$AB_BASE")" != "$(cut -d' ' -f3- <<< "$AB_CAND")" ] || { expect varena-ab FAIL "both arms are $(cut -d' ' -f3- <<< "$AB_BASE")"; return 0; }
# ab_on <cozy> <tensorfs> <cozy version> <tensorfs version> <GiB|off>: a restart onto that pair under that cap, as `machine show` says.
ab_on() {
  local want=0 show said lim i
  [ "$5" = off ] || want=$(($5 << 30))
  "$CLI" --json machine update "$M" --tensord "$1" --tensorfs "$2" --gpu-memory-limit "$([ "$5" = off ] && echo off || echo "${5}GiB")" -y \
    > ab-update.json 2> ab-update.log || { log "varena-ab: update to $3/$4 at $5: $(tail -c 300 ab-update.log | tr '\n' ' ')"; return 1; }
  for i in $(seq 36); do
    show=$("$CLI" --json machine show "$M" 2> /dev/null)
    said=$(jq -r '"\(.status.version // "")/\(.status.tensorfs // "")"' <<< "$show")
    lim=$(jq -r '[.status.gpus[]? | .memory_limit_bytes // 0 | tostring] | unique | join(",")' <<< "$show")
    [ "$said" = "$3/$4" ] && [ "$lim" = "$want" ] && return 0
    sleep 5
  done
  log "varena-ab: machine show says ${said:-?} at ${lim:-?} bytes, not $3/$4 at $want"
  return 1
}
# ab_model <model> <cell prefix> <suffix>: its cold and warm run; each its own prompt and seed, the same in both arms.
# SDXL runs once more after a pause, when idle work has had its time (tensorfs-ab.sh).
ab_model() {
  local m=$1 n=$2 sfx=$3 i v s
  local -a when=("at dusk" "in light rain" "under clear skies")
  for i in 0 1 2; do
    [ "$i" != 2 ] || [ "$m" = sdxl ] || continue
    [ "$i" != 2 ] || sleep 30
    v=${when[$i]}
    case $m in
      sdxl) s=$((R + 700 + i))
        cell "$n-$i$sfx" "$(pkg sdxl generate)" prompt="A brass astrolabe on a sailor's chart table $v, warm lamplight" \
          megapixels:=1 steps:=20 guidance:=7 seed:=$s ;;
      klein) s=$((R + 710 + i))
        cell "$n-$i$sfx" "$(pkg flux.3-klein generate_turbo)" prompt="A ceramic teapot on a windowsill $v, studio photo" \
          prompt_upsampling:=false seed:=$s ;;
      qwen4mp) s=$((R + 720 + i))
        cell "$n-$i$sfx" "$(pkg qwen-image-2 generate)" prompt="A vintage tram crossing a cobbled square $v, fine architectural detail" \
          megapixels:=4 prompt_upsampling:=false seed:=$s ;;
      h3) s=$((R + 730 + i))
        cell "$n-$i$sfx" "$(pkg minimax-h3 generate)" \
          "beats:=[{\"description\":\"[Shot 1]\\nA paper boat drifts down a rain-filled gutter past glowing shop windows $v. The camera follows at street level as drops ripple the water.\",\"duration\":15}]" \
          soundscape="Rain on pavement, distant traffic." turbo:=true seed:=$s ;;
      ideogram) s=$((R + 740 + i))
        cell "$n-$i$sfx" "$(pkg ideogram-4 generate)" \
          "prompt={\"high_level_description\":\"A bakery storefront $v with a sign that reads FRESH BREAD\",\"style_description\":\"photograph, soft daylight\",\"compositional_deconstruction\":{\"background\":\"a brick shop front with a wide window of loaves and a green door\",\"elements\":[{\"type\":\"text\",\"bbox\":[90,180,230,820],\"text\":\"FRESH BREAD\",\"desc\":\"a painted wooden sign above the window\"}]}}" \
          megapixels:=1 aspect_ratio=1:1 steps:=48 guidance:=7 prompt_upsampling:=false seed:=$s ;;
    esac
  done
}
# ab_arm <arm> <cap> <model> <prefix> <suffix>: a restart onto that arm, then the model alone.
ab_arm() {
  local arm=$1; shift
  # shellcheck disable=SC2086
  if ab_on $arm "$1"; then ab_model "$2" "$3" "$4"; else expect "$3$4" FAIL "$(cut -d' ' -f3- <<< "$arm") did not go on at $1"; fi
}
log "varena-ab: base $(cut -d' ' -f3- <<< "$AB_BASE") against candidate $(cut -d' ' -f3- <<< "$AB_CAND")"
# Which arm goes first alternates by level: the second one's cold run finds the weights in the page cache.
for lvl in off 8 6; do
  for m in h3 qwen4mp sdxl klein ideogram; do
    [ "$m" != ideogram ] || [ "$lvl" = off ] || continue
    [[ " ${AB_SKIP:-} " != *" ab-$m-$lvl "* ]] || continue
    [ "$lvl" = 8 ] || ab_arm "$AB_BASE" "$lvl" "$m" "ab-$m-$lvl" -released
    ab_arm "$AB_CAND" "$lvl" "$m" "ab-$m-$lvl" ''
    [ "$lvl" != 8 ] || ab_arm "$AB_BASE" "$lvl" "$m" "ab-$m-$lvl" -released
  done
done
# Then the bench's case, uncapped: SDXL and Qwen-Image 4 MP leave their executors idle beside Ideogram 4.
for arm in released candidate; do
  sfx=; a=$AB_CAND; [ "$arm" = candidate ] || { sfx=-released; a=$AB_BASE; }
  # shellcheck disable=SC2086
  ab_on $a off || { expect "ab-peers-$arm" FAIL "did not go on"; continue; }
  for m in sdxl qwen4mp ideogram; do ab_model "$m" "ab-peers-$m" "$sfx"; done
done
