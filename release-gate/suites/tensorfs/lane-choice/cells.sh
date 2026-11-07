# Case `lane-choice` of suite tensorfs (xs-154; design lane-choice.md): the machine picks each lane from the release's
# lanes, its GPUs, its store and the slot's tolerances. Only by name, on a GPU without FP4 tensor cores (sm80-sm90):
#   gate.sh rtx-4090 --suite tensorfs --only lane-choice --tensord <cozy> --tensorfs <wheel>
# Sourced by ../cells.sh (tfs_step, $T, $M, $CLI, expect). The arms, in this order; memory, min-bits and offline FAIL on
# 0.21.1:
#   lc-memory   min_bits 8 under an 8 GiB software cap: Klein's transformer (int8 9.5 GB, nvfp4 7.3 GB) downloads at its
#               8-bit lane. 0.21.1 serves nvfp4: the Hub's Select lowered lanes to free VRAM (select.go:376-414).
#   lc-cached   its upsampler's nvfp4 lane downloaded by label, then the model by no lane: the int8 lane, fetched. A
#               cached worse lane only breaks ties (R1)
#   lc-pin      a label the release lacks: refused, naming every lane (R2)
#   lc-min-bits Klein's call: its upsampler (min_bits=4, decode) at a 4-bit lane, its transformer at 8 bits. Escaped to
#               0.21.1, which serves the upsampler int8: Select fixed 8 bits and decode off (select.go:327,332)
#   lc-offline  on this host, `tfs` with the Hub unreachable over a store holding SDXL's fp8 and int8 lanes: int8, the best
#               on the CPU (fp8 is weight-only there), with the warning; 0.21.1 refuses, "name a lane". Over an empty
#               store: refused, naming `cozy model download` (R4)
# The capacity rule at min_bits 4 (a non-decode slot taking 4 bits when its 8-bit lane is larger than the GPU) has no
# first-party slot to run: every min_bits=4 slot decodes.
LC_KLEIN=black-forest-labs/flux-3-klein LC_UP=black-forest-labs/flux-3-klein-upsampler LC_SDXL=stabilityai/sdxl
LC_FOUR='^(nvfp4|mxfp4|int4|w4a8|svdq-nvfp4)$' LC_EIGHT='^(int8|fp8)$'
# lc_row <name> <repository>: the download's result row for the model, as `lane precision moved-bytes`.
lc_row() {
  run_get "$T" "$1" "[.result.models[]? | select(.repository == \"$2\") | \"\\(.lane) \\(.precision) \\((.bytes.hub // 0) + (.bytes.volume // 0))\"][0]"
}
lc_verdict() { expect "$1" "$([ -z "$3" ] && echo PASS || echo FAIL)" "${3:-$2}"; }

if [ "$("$CLI" --json machine show "$M" 2>/dev/null | jq '.status.gpus | length')" -gt 0 ]; then
  "$CLI" machine update "$M" --gpu-memory-limit 8GiB -y > "$T/lc-cap.log" 2>&1
  tfs_step lc-memory model download "$LC_KLEIN" --on "$M" -y
  "$CLI" machine update "$M" --gpu-memory-limit off -y > "$T/lc-cap-off.log" 2>&1
  read -r lane precision _ <<< "$(lc_row lc-memory "$LC_KLEIN")"
  bad=; [[ $precision =~ $LC_EIGHT ]] || bad="under an 8 GiB cap $LC_KLEIN served ${lane:-nothing} (${precision:-?}): memory lowered it"
  lc_verdict lane-choice-memory "$LC_KLEIN under an 8 GiB cap: $lane ($precision)" "$bad"

  tfs_step lc-cached-four model download "$LC_UP" --lane nvfp4 --on "$M" -y
  tfs_step lc-cached model download "$LC_UP" --on "$M" -y
  read -r held _ <<< "$(lc_row lc-cached-four "$LC_UP")"
  read -r lane precision moved <<< "$(lc_row lc-cached "$LC_UP")"
  bad=
  [ "$held" = nvfp4 ] || bad="--lane nvfp4 served ${held:-nothing}; "
  [[ $precision =~ $LC_EIGHT ]] && [ "${moved:-0}" -gt 0 ] || bad+="with nvfp4 held, no lane served ${lane:-nothing} ($precision, ${moved:-0} B moved)"
  lc_verdict lane-choice-cached "nvfp4 held, then $lane ($precision), $moved B fetched" "$bad"

  tfs_step lc-pin model download "$LC_UP" --lane no-such-lane --on "$M" -y
  said=$(run_get "$T" lc-pin '.reason.message // ""')
  bad=; [ "$(run_get "$T" lc-pin .status)" != completed ] && [[ $said == *bf16* && $said == *int8* && $said == *nvfp4* ]] ||
    bad="--lane no-such-lane: $(run_get "$T" lc-pin .status), \"$said\" names not every lane"
  lc_verdict lane-choice-pin "refused: $said" "$bad"

  tfs_step lc-min-bits run "$(pkg flux-3-klein generate_turbo)" prompt="a lighthouse at dusk, lane-choice $R" seed:=$((R + 154)) \
    steps:=4 prompt_upsampling:=false --on "$M"
  models=$(run_get "$T" lc-min-bits '[.. | .models? | arrays | .[] | objects | select(.slot != null) | "\(.slot)=\(.precision)"] | unique | join(" ")')
  up=$(tr ' ' '\n' <<< "$models" | sed -n 's/^upsampler=//p' | head -1) main=$(tr ' ' '\n' <<< "$models" | sed -n 's/^model=//p' | head -1)
  bad=
  [ "$(run_get "$T" lc-min-bits .status)" = completed ] || bad="Klein's call $(run_get "$T" lc-min-bits .status); "
  [[ $up =~ $LC_FOUR ]] || bad+="the upsampler (min_bits=4, decode) served ${up:-nothing}; "
  [[ $main =~ $LC_EIGHT ]] || bad+="the transformer served ${main:-nothing}"
  lc_verdict lane-choice-min-bits "Klein: ${models:-no models recorded}" "$bad"
else
  log "tensorfs lane-choice: $M has no GPU: its machine arms need a GPU SKU"
fi

s=$T/lc-store e=$T/lc-empty
rm -rf "$s" "$e"
for l in fp8 int8; do
  TENSORFS_HOME=$s uv tool run -q --from "${TWF:-tensorfs}" tfs download "$LC_SDXL" --lane "$l" --dir "$T/lc-$l" >"$T/lc-get-$l.out" 2>&1
done
TENSORFS_HOME=$s uv tool run -q --from "${TWF:-tensorfs}" tfs --endpoint http://127.0.0.1:9 --json inspect "$LC_SDXL" \
  >"$T/lc-offline.json" 2>"$T/lc-offline.err"
lane=$(jq -r '.lane // empty' "$T/lc-offline.json" 2>/dev/null)
bad=; [ "$lane" = int8 ] && grep -q "Hub unreachable: served cached int8" "$T/lc-offline.err" ||
  bad="Hub unreachable over fp8 and int8 held: ${lane:-refused} $(tail -c 300 "$T/lc-offline.json")"
lc_verdict lane-choice-offline "Hub unreachable over fp8 and int8 held: int8, warned" "$bad"
TENSORFS_HOME=$e uv tool run -q --from "${TWF:-tensorfs}" tfs --endpoint http://127.0.0.1:9 --json inspect "$LC_SDXL" \
  >"$T/lc-empty.json" 2>"$T/lc-empty.err" && bad="Hub unreachable over an empty store: served $(jq -r .lane "$T/lc-empty.json")" ||
  { grep -q "cozy model download $LC_SDXL" "$T/lc-empty.json" && bad= || bad="refused, not naming the download: $(tail -c 300 "$T/lc-empty.json")"; }
lc_verdict lane-choice-offline-empty "Hub unreachable over an empty store: refused, naming the download" "$bad"
rm -rf "$s" "$e" "$T"/lc-fp8 "$T"/lc-int8
