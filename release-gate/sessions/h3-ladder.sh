# h3-ladder: one multi-GPU rental on the production cohort and recorded H3 requests (prepared references, fixed
# seeds), run as cells in the given order, each one serving choice (lane, attention pin, LoRA serving) away from its
# request's gold cell. Every cell of a request must compile the same prompt over the same references; each is scored
# against its gold (per-frame PSNR and SSIM, each window's mean), with the ghost score and window strips for review.
# A cell that would outrun the cap is skipped, never started, so order cells most informative first.
# Optional `refill`: a warm call after another model displaced its pages (`refilled`) must bake its LoRA into every
# refilled block and decode the frames of the same call warm (`warm`).
# The requests and media are the owner's: they never enter OUT. On a hosted runner it refuses; from the owner's computer:
#   OUT=… SKU=… DISK=… CAP=… RENT=false PARAMS="$(cat params.json)" bash release-gate/sessions/h3-ladder.sh
# params:
#   requests  {"<id>": {"target", "request" (payload file with a seed), "assets": [{"field", "path", "sha256"}],
#             "windows": "a-b,…", "recorded": {"path", "sha256"} (optional), "expect": {"prompt", "references"} (optional)}}
#   cells     [{"name", "request", "lane", "pin" ("" for the default route), "lora_mode" ("" | per_step | bake),
#             "gold" (one per request), "est_s" (expected seconds)}]
#   gpus (4), cohort {"runtime" (required, on PyPI), "tensorfs", "tensord", "image" (each asserted if given)}
#   python, ghost: the ghost detector (h3-video-forensics' ghost.py, standalone) and an interpreter with numpy/scipy/cv2
#   refill {"warm", "refilled"} (optional); media: directory for videos, strips and per-cell payloads (outside OUT)
#   margin_s (420): kept for the last comparisons and the end; traffic_gb (200), need_s (3000), stall_s (1800)
. "$(dirname "$0")/lib.sh"
[ "${GITHUB_ACTIONS:-}" != true ] || fail "private request media: run this session from the owner's computer, not a hosted runner"
REQS=$(param requests '{}') CELLS=$(param cells '[]') GPUS=$(param gpus 4) COHORT=$(param cohort '{}') REFILL=$(param refill '{}')
MEDIA=$(param media) PY=$(param python) GHOST=$(param ghost) MARGIN=$(param margin_s 420) STALL_S=$(param stall_s 1800)
command -v ffmpeg >/dev/null && command -v sha256sum >/dev/null || fail "no ffmpeg/sha256sum"
[ -n "$MEDIA" ] && [[ $MEDIA != "$OUT"* ]] || fail "media must be a directory outside OUT"
mkdir -p "$MEDIA/requests"
[[ $GPUS =~ ^[1-8]$ ]] || fail "gpus: $GPUS"
RT=$(jq -r '.runtime // empty' <<<"$COHORT")
[[ $RT =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "cohort.runtime '$RT' is not a release version"
"$PY" -I -c 'import numpy, scipy, cv2' 2>/dev/null && [ -f "$GHOST" ] || fail "ghost detector: $PY $GHOST"
declare -A TARGET REQ WIN GOLD
mapfile -t ids < <(jq -r 'keys[]' <<<"$REQS")
for id in "${ids[@]}"; do
  spec=$(jq -c --arg i "$id" '.[$i]' <<<"$REQS")
  [[ $id =~ ^[A-Za-z0-9_-]+$ ]] || fail "request id $id"
  TARGET[$id]=$(jq -r .target <<<"$spec") REQ[$id]=$(jq -r .request <<<"$spec") WIN[$id]=$(jq -r .windows <<<"$spec")
  [[ ${TARGET[$id]} =~ ^[a-z0-9-]+/[a-z0-9_]+@[0-9.]+$ && ${WIN[$id]} =~ ^[0-9]+-[0-9]+(,[0-9]+-[0-9]+)*$ ]] || fail "$id target/windows"
  jq -e 'type=="object" and has("prompt") and has("seed") and (has("lora_mode") | not)' "${REQ[$id]}" >/dev/null 2>&1 ||
    fail "$id request needs a prompt and a seed, and no lora_mode of its own"
  declare -ga "ASSETS_$id=()"
  while read -r a; do
    f=$(jq -r .field <<<"$a") p=$(jq -r .path <<<"$a") h=$(jq -r .sha256 <<<"$a")
    [[ $f =~ ^[a-z_]+(\.[0-9a-z_]+)+$ ]] && [ "$(sha256sum "$p" 2>/dev/null | cut -c1-64)" = "$h" ] || fail "$id asset $f: $p is not sha256:$h"
    eval "ASSETS_$id+=(--asset \"\$f=\$p\")"
  done < <(jq -c '.assets[]' <<<"$spec")
  r=$(jq -r '.recorded.path // empty' <<<"$spec")
  [ -z "$r" ] || [ "$(sha256sum "$r" | cut -c1-64)" = "$(jq -r .recorded.sha256 <<<"$spec")" ] || fail "$id recorded video"
  sha256sum "${REQ[$id]}" | cut -c1-64 > "$OUT/$id-request.sha256.txt"  # the payloads stay out of OUT
done
mapfile -t names < <(jq -r '.[].name' <<<"$CELLS")
[ "${#names[@]}" -gt 0 ] || fail "no cells"
for k in "${names[@]}"; do
  c=$(jq -c --arg k "$k" 'map(select(.name == $k)) | first' <<<"$CELLS") id=$(jq -r .request <<<"$c")
  [[ $k =~ ^[A-Za-z0-9_-]+$ ]] && [ "$(jq --arg k "$k" 'map(select(.name == $k)) | length' <<<"$CELLS")" = 1 ] || fail "cell names must be unique words: $k"
  [ -n "${TARGET[$id]:-}" ] || fail "cell $k names request $id, which is not there"
  jq -e '(.lane | test("^[a-z0-9-]+$")) and ((.pin // "") | test("^([a-z_/]+=[a-z0-9-]+)?$")) and ((.lora_mode // "") | IN("", "per_step", "bake"))
    and (.est_s | type == "number")' <<<"$c" >/dev/null || fail "cell $k: $c"
  if [ "$(jq -r '.gold // false' <<<"$c")" = true ]; then [ -z "${GOLD[$id]:-}" ] || fail "$id has two gold cells"; GOLD[$id]=$k; fi
  jq --arg m "$(jq -r '.lora_mode // ""' <<<"$c")" 'if $m == "" then . else .lora_mode = $m end' "${REQ[$id]}" > "$MEDIA/requests/$k.json"
  say "cell $k: ${TARGET[$id]} --lane $(jq -r .lane <<<"$c") $(jq -r 'if (.pin // "") == "" then "default route" else "--attention-kernel " + .pin end' <<<"$c") lora=$(jq -r '.lora_mode // "" | if . == "" then "default" else . end' <<<"$c") ~$(jq -r .est_s <<<"$c") s$( [ "${GOLD[$id]:-}" = "$k" ] && echo ' GOLD')"
done
for id in "${ids[@]}"; do [ -n "${GOLD[$id]:-}" ] || fail "$id has no gold cell"; done
[ "$REFILL" = '{}' ] || jq -e --argjson n "$(jq -c 'map(.name)' <<<"$CELLS")" '(.warm as $w | $n | index($w)) and (.refilled as $r | $n | index($r))' <<<"$REFILL" >/dev/null ||
  fail "refill names cells that are not there"
curl -fsS -o /dev/null "https://pypi.org/pypi/cozy-runtime/$RT/json" 2>/dev/null || fail "cozy-runtime $RT is not on PyPI yet"
for id in "${ids[@]}"; do
  pkg=${TARGET[$id]%%/*} rel=${TARGET[$id]##*@}
  cozy --json package info "$pkg" 2>/dev/null | jq -e --arg r "$rel" '.releases[] | select(.release == $r and (.yanked | not))' >/dev/null ||
    fail "$pkg@$rel is not a live release on the Hub"
done
total=$(jq '[.[].est_s] | add' <<<"$CELLS")
say "live: cozy-runtime $RT, $(printf '%s ' "${TARGET[@]}"); ${#names[@]} cells, ~$((total / 60)) min of cells"

# facts <cell>: what its record shows the DiT call ran on and with, one line of JSON.
facts() {
  local j=$OUT/$1.json t=$OUT/$1.show.txt
  jq -c --arg prompt "$(grep -oE 'h3 encoder prompt chars=[0-9]+ digest=[0-9a-f]+' "$t" | tail -1 | awk -F'digest=' '{print $2}')" \
    --arg refs "$(grep -oE 'h3 fixed reference digest=sha256:[0-9a-f]+' "$t" | sed 's/.*sha256://' | paste -sd, -)" \
    --arg sizing "$(grep -oE 'h3 reference sizing .*video_tokens=[0-9]+' "$t" | tail -1)" \
    --arg sched "$(grep -oE 'timestep_plan_digest=[0-9a-f]{12}' "$t" | tail -1)" '
    . as $run | ([.calls[] | select(.function | test("^generate_")) | .measurements] | last) as $m | {
      status: $run.status, prompt: $prompt, references: ($refs | split(",")), sizing: $sizing, schedule: $sched,
      software: $m.software, degree: $m.execution.degree,
      models: [$m.models[]? | "\(.slot)=\(.repository)@\(.release) \(.lane) \(.manifest[7:19])"],
      route: [$m.observations[]? | select(.name == "attention.selected" and (.fields.component | test("transformer"))) | "\(.value)[\(.fields.dense_reference)] pin=\(.fields.pin)"],
      sol: [$m.observations[]? | select(.name == "attention.sol.calls") | .fields | del(.dense_reference)],
      ready: [$m.observations[]? | select(.name == "kernel.ready") | "\(.value)@\(.fields.stage // "")/\(.fields.step // "")"] | unique,
      steps_s: [$m.attribution.steps.denoise.series[]?[1] / 1000 | . * 10 | round / 10],
      denoise_s: (($m.attribution.stages.denoise.total_ms // 0) / 1000 | floor), exec_s: (($run.execution_ms // 0) / 1000 | floor),
      fills: [$m.memory.stats.varena[]? | {gpu, dit: [.stats.trace[] | select(.set | test("/transformer(_ref)?#"))
                | select(.origin != "hit") | {block, origin}], overlay: [.stats.trace[] | select(.set | test("_turbo#"))] | length}]}' "$j"
}
video() { jq -r '.output[]?.path' "$OUT/$1.json" 2>/dev/null | grep -E '\.mp4$' | head -1; }
frames_md5() { ffmpeg -nostdin -v error -i "$1" -map 0:v:0 -f framemd5 - | grep -v '^#' | md5sum | cut -c1-16; }
# score <a.mp4> <gold.mp4> <tag> <windows>: per-frame PSNR and SSIM into OUT; "PSNR mean/min, SSIM mean/min" and windows.
score() {
  ffmpeg -nostdin -v error -i "$1" -i "$2" -lavfi "[0:v][1:v]psnr=stats_file=$OUT/psnr-$3.txt;[0:v][1:v]ssim=stats_file=$OUT/ssim-$3.txt" -f null - ||
    { echo "score failed"; return; }
  awk -v w="$4" 'FNR == 1 {file++} file == 1 {for(i=1;i<=NF;i++){split($i,kv,":"); if(kv[1]=="n")n=kv[2]; if(kv[1]=="psnr_avg")p=(kv[2]=="inf"?99:kv[2])}; P[n-1]=p}
    file == 2 {for(i=1;i<=NF;i++){split($i,kv,":"); if(kv[1]=="n")n=kv[2]; if(kv[1]=="All")s=kv[2]}; S[n-1]=s}
    END{for(f in P){c++; ps+=P[f]; if(c==1||P[f]<pm){pm=P[f]; pa=f}; ss+=S[f]; if(c==1||S[f]<sm){sm=S[f]; sa=f}}
      printf "PSNR %.2f dB (min %.2f at f%d), SSIM %.4f (min %.4f at f%d)", ps/c, pm, pa, ss/c, sm, sa; k=split(w,ws,",")
      for(j=1;j<=k;j++){split(ws[j],r,"-"); tp=0; ts=0; q=0; for(x=r[1];x<=r[2];x++) if(x in P){tp+=P[x]; ts+=S[x]; q++}
        if(q) printf "; f%s %.2f dB / %.4f", ws[j], tp/q, ts/q}}' "$OUT/psnr-$3.txt" "$OUT/ssim-$3.txt"
}
strips() {  # strips <tag> <mp4> <windows>: each window as one tiled PNG in MEDIA
  local w a b
  for w in ${3//,/ }; do a=${w%-*} b=${w#*-}
    ffmpeg -nostdin -v error -y -i "$2" -vf "select='between(n\,$a\,$b)',scale=448:-1,tile=$((b - a + 1))x1" -frames:v 1 "$MEDIA/$1-f$w.png" || say "strip $1 f$w failed"
  done
}
ghost() { nice -n 19 "$PY" -I "$GHOST" "$1" "$3" > "$OUT/ghost-$2.json" 2>> "$OUT/ghost.err" && jq -c '{mean, p95, windows}' "$OUT/ghost-$2.json"; }
same() {  # same <a> <b>: both compiled one request
  jq -e --slurpfile a "$OUT/$1.facts.json" '[.prompt, .references, .sizing, .schedule, .software.runtime, .degree]
    == ($a[0] | [.prompt, .references, .sizing, .schedule, .software.runtime, .degree])' "$OUT/$2.facts.json" >/dev/null || fail "$1 and $2 did not run the same request"
}
# cell <name>: one run, judged by its record: completed, its lane, its route, its video.
cell() {
  local k=$1 c id lane pin N code v
  c=$(jq -c --arg k "$k" 'map(select(.name == $k)) | first' <<<"$CELLS") id=$(jq -r .request <<<"$c") lane=$(jq -r .lane <<<"$c") pin=$(jq -r '.pin // ""' <<<"$c")
  local -n assets="ASSETS_$id"
  step "$k" run "${TARGET[$id]}" --input "$MEDIA/requests/$k.json" "${assets[@]}" --lane "$lane" --gpus "$GPUS" \
    ${pin:+--attention-kernel "$pin"} --on "$NAME" --out "$MEDIA/$k"; code=$?
  N=$(runno "$k"); [ -n "$N" ] || fail "$k named no run (exit $code)"
  cozy run show "$N" --once --json > "$OUT/$k.json" 2>/dev/null; cozy run show "$N" --once > "$OUT/$k.show.txt" 2>&1
  facts "$k" > "$OUT/$k.facts.json" || fail "$k: no record"
  say "$k run $N (exit $code): $(jq -c 'del(.fills, .ready)' "$OUT/$k.facts.json")"
  [ "$(jq -r .status "$OUT/$k.facts.json")" = completed ] || fail "$k did not complete: $(jq -r '.reason.message // .status' "$OUT/$k.json" | head -c 600)"
  jq -e --arg l "$lane" 'any(.models[]; test(" " + $l + " "))' "$OUT/$k.facts.json" >/dev/null || fail "$k did not serve lane $lane"
  case "$pin" in
    "") ;;
    *=sol-attn) jq -e '.route[0] | startswith("sol-attn[sdpa")' "$OUT/$k.facts.json" >/dev/null || fail "$k did not serve Sol with SDPA dense rows" ;;
    *) jq -e --arg p "${pin#*=}" '.route[0] | startswith($p + "[")' "$OUT/$k.facts.json" >/dev/null || fail "$k did not serve ${pin#*=}" ;;
  esac
  v=$(video "$k"); [ -s "$v" ] || fail "$k: no video"
  strips "$k" "$v" "${WIN[$id]}"; say "$k ghost: $(ghost "$v" "$k" "${WIN[$id]}")"
}
cohort() {
  cozy machine show "$RID" --json > "$OUT/show-cohort.json" 2>&1
  jq -e --argjson c "$COHORT" '.status.runtime==$c.runtime and (($c.tensorfs // .status.tensorfs) == .status.tensorfs)
    and (($c.tensord // .status.version) == .status.version)
    and (($c.image // .rental.base_worker_image_digest) == .rental.base_worker_image_digest)' "$OUT/show-cohort.json" >/dev/null ||
    fail "$NAME runs $(jq -c '{v: .status.version, rt: .status.runtime, tfs: .status.tensorfs, image: .rental.base_worker_image_digest}' "$OUT/show-cohort.json")"
  say "ASSERT $NAME runs $(jq -c '{v: .status.version, rt: .status.runtime, tfs: .status.tensorfs, image: .rental.base_worker_image_digest}' "$OUT/show-cohort.json") on $GPUS GPUs"
}
quote "$(param traffic_gb 200)" "$(param need_s 3000)"
T0=$(date +%s)
rent
grep -q "^GPU $((GPUS - 1)) " "$OUT/show.txt" || fail "$NAME does not list GPU $((GPUS - 1))"
cohort
DEADLINE=$(( T0 + CAP_S - MARGIN ))  # the guard ends the pod CAP_S after the order

ran=() skipped=()
for k in "${names[@]}"; do
  est=$(jq -r --arg k "$k" '.[] | select(.name == $k) | .est_s' <<<"$CELLS")
  if [ $(( $(date +%s) + est )) -gt "$DEADLINE" ]; then skipped+=("$k"); say "SKIP $k: ~${est}s would outrun the cap"; continue; fi
  cell "$k"; ran+=("$k")
done
for id in "${ids[@]}"; do
  g=${GOLD[$id]}
  printf '%s\n' "${ran[@]}" | grep -qx "$g" || { say "$id: gold $g did not run: no scores"; continue; }
  spec=$(jq -c --arg i "$id" '.[$i]' <<<"$REQS")
  for k in "${ran[@]}"; do
    [ "$(jq -r --arg k "$k" '.[] | select(.name == $k) | .request' <<<"$CELLS")" = "$id" ] && [ "$k" != "$g" ] || continue
    same "$g" "$k"; say "$k vs gold $g: $(score "$(video "$k")" "$(video "$g")" "$k-$g" "${WIN[$id]}")"
  done
  p=$(jq -r .prompt "$OUT/$g.facts.json")
  [ "$p" = "$(jq -r '.expect.prompt // empty' <<<"$spec")" ] && [ "$(jq -c .references "$OUT/$g.facts.json")" = "$(jq -c '.expect.references // []' <<<"$spec")" ] &&
    say "ASSERT $id compiled its recorded prompt over its recorded references" || say "WARN $id's prompt or references differ from the recorded ones"
  r=$(jq -r '.recorded.path // empty' <<<"$spec")
  [ -z "$r" ] || { say "recorded $id vs gold $g: $(score "$r" "$(video "$g")" "recorded-$id" "${WIN[$id]}")"; strips "recorded-$id" "$r" "${WIN[$id]}"; say "recorded $id ghost: $(ghost "$r" "recorded-$id" "${WIN[$id]}")"; }
done
if [ "$REFILL" != '{}' ]; then
  w=$(jq -r .warm <<<"$REFILL") f=$(jq -r .refilled <<<"$REFILL")
  if printf '%s\n' "${ran[@]}" | grep -qx "$f"; then
    # Per GPU: blocks the refilled call filled from the store (block 0 holds no LoRA site) and its overlay reads beyond the warm call's.
    jq -c --slurpfile a "$OUT/$w.facts.json" '[.fills[] | . as $g | ($a[0].fills[] | select(.gpu == $g.gpu)) as $x |
      {gpu, refilled: ([$g.dit[] | select(.origin == "store" and .block != 0)] | length), derived: ([$g.dit[] | select(.origin != "store")] | length),
       reads: ($g.overlay - $x.overlay)}]' "$OUT/$f.facts.json" > "$OUT/refill.json"
    say "refill ($f after $w) per GPU: $(cat "$OUT/refill.json")"
    [ "$(jq '[.[].refilled] | add // 0' "$OUT/refill.json")" -gt 0 ] || fail "no Turbo DiT page was displaced: $f refilled nothing"
    jq -e 'all(.[]; .reads >= 6 * .refilled)' "$OUT/refill.json" >/dev/null || fail "a refilled LoRA block was not baked (fewer than 6 factor reads per block)"
    fid=$(jq -r --arg k "$f" '.[] | select(.name == $k) | .request' <<<"$CELLS")
    if [ "$(frames_md5 "$(video "$w")")" = "$(frames_md5 "$(video "$f")")" ]; then say "ASSERT $f decodes the frames of $w"
    else
      p=$(score "$(video "$f")" "$(video "$w")" "$f-$w" "${WIN[$fid]}"); say "$f vs $w: frames differ: $p"
      awk -v m="$(awk '{print $2}' <<<"$p")" 'BEGIN{exit !(m >= 45)}' || fail "$f differs from $w below 45 dB: refilled blocks do not serve the warm call's weights"
    fi
  else say "refill: $f did not run"; fi
fi
say "ran ${ran[*]:-none}; skipped ${skipped[*]:-none}; strips and videos: $MEDIA"
[ "${#skipped[@]}" = 0 ] || say "WARN ${#skipped[@]} cells skipped by the cap"
