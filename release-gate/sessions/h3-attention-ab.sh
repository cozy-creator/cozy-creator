# h3-attention-ab: one multi-GPU rental on the cohort that served a recorded H3 request, that request replayed with its
# prepared references and seed, and cells that differ only in the DiT's attention route: as served (R0), Sol sparse with
# exact SDPA dense rows (transformer_ref=sol-attn), exact SDPA everywhere (transformer_ref=sdpa). Each cell's record must
# show the same compiled prompt and reference digests; the cells' videos are compared frame by frame with each other
# and with the recorded video, and strips of the named frame windows are cut for review.
# The request and its media are the owner's: they never enter OUT's artifact. On a hosted runner it refuses; run it
# from the owner's computer: OUT=… SKU=… DISK=… CAP=… RENT=false PARAMS='{…}' bash release-gate/sessions/h3-attention-ab.sh
# params:
#   request    JSON payload file (the function's input, the media fields left to `assets`)
#   assets     [{"field", "path", "sha256"}]: each attached file, checked before the quote
#   target     the function (minimax-h3/ref2va@0.1.0)
#   lane       the main model's lane (int8)
#   gpus       GPUs on the machine and for every cell (4)
#   cells      [{"name", "pin"}]: run in order; pin "" serves the default route
#   cohort     {"runtime", "tensorfs", "tensord", "image"}: what the machine must already run
#   expect     {"prompt", "references": [sha256…], "dense": [served route of R0, informative]}: the recorded request
#   recorded   {"path", "sha256"}: the recorded video R0 is compared with
#   media      directory for videos and strips (outside OUT)
#   windows    frame windows for strips ("55-65,248-273")
#   traffic_gb GB the provider bills (150); need_s seconds the cap must buy (3600); stall_s (1500)
. "$(dirname "$0")/lib.sh"
[ "${GITHUB_ACTIONS:-}" != true ] || fail "private request media: run this session from the owner's computer, not a hosted runner"
REQ=$(param request) TARGET=$(param target minimax-h3/ref2va@0.1.0) LANE=$(param lane int8) GPUS=$(param gpus 4)
ASSETS=$(param assets '[]') CELLS=$(param cells '[{"name":"R0","pin":""},{"name":"R1","pin":"transformer_ref=sol-attn"},{"name":"R2","pin":"transformer_ref=sdpa"}]')
COHORT=$(param cohort '{}') EXPECT=$(param expect '{}') RECORDED=$(param recorded '{}') MEDIA=$(param media) WINDOWS=$(param windows 55-65,248-273)
STALL_S=$(param stall_s 1500)
command -v ffmpeg >/dev/null && command -v sha256sum >/dev/null || fail "no ffmpeg/sha256sum"
[ -n "$MEDIA" ] && [[ $MEDIA != "$OUT"* ]] || fail "media must be a directory outside OUT"
mkdir -p "$MEDIA"
[ -s "$REQ" ] && jq -e 'type=="object" and has("prompt")' "$REQ" >/dev/null || fail "request $REQ is not a payload with a prompt"
[[ $GPUS =~ ^[1-8]$ ]] || fail "gpus: $GPUS"
[[ $WINDOWS =~ ^[0-9]+-[0-9]+(,[0-9]+-[0-9]+)*$ ]] || fail "windows: $WINDOWS"
args=()
while read -r a; do
  f=$(jq -r .field <<<"$a") p=$(jq -r .path <<<"$a") h=$(jq -r .sha256 <<<"$a")
  [[ $f =~ ^[a-z_]+(\.[0-9a-z_]+)+$ ]] || fail "asset field $f"
  [ "$(sha256sum "$p" 2>/dev/null | cut -c1-64)" = "$h" ] || fail "asset $f: $p is not sha256:$h"
  args+=(--asset "$f=$p")
done < <(jq -c '.[]' <<<"$ASSETS")
[ "${#args[@]}" -gt 0 ] || fail "no assets"
rec=$(jq -r '.path // empty' <<<"$RECORDED")
[ -z "$rec" ] || [ "$(sha256sum "$rec" | cut -c1-64)" = "$(jq -r .sha256 <<<"$RECORDED")" ] || fail "recorded video $rec does not match its sha256"
mapfile -t names < <(jq -r '.[].name' <<<"$CELLS")
[ "${#names[@]}" -ge 2 ] || fail "fewer than two cells"
for k in "${names[@]}"; do
  [[ $k =~ ^[A-Za-z0-9_-]+$ ]] && [ "$(jq --arg k "$k" 'map(select(.name==$k)) | length' <<<"$CELLS")" = 1 ] || fail "cell names must be unique words: $k"
  pin=$(jq -r --arg k "$k" '.[] | select(.name==$k) | .pin' <<<"$CELLS")
  [ -z "$pin" ] || [[ $pin =~ ^[a-z_/]+=[a-z0-9-]+$ ]] || fail "cell $k pin: $pin"
  say "cell $k: cozy run $TARGET --input request.json $((${#args[@]} / 2)) assets --lane $LANE --gpus $GPUS ${pin:+--attention-kernel $pin}"
done
jq -e '.runtime and .tensorfs and .tensord and .image' <<<"$COHORT" >/dev/null || fail "cohort needs runtime, tensorfs, tensord, image"
sha256sum "$REQ" | cut -c1-64 > "$OUT/request.sha256.txt"  # the payload stays out of OUT: its digest only
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
      denoise_s: (($m.attribution.stages.denoise.total_ms // 0) / 1000 | floor)}' "$j"
}
video() { jq -r '.output[]?.path' "$OUT/$1.json" 2>/dev/null | grep -E '\.mp4$' | head -1; }
# psnr <a.mp4> <b.mp4> <tag>: per-frame PSNR into OUT, then "mean min frame-of-min" and each window's mean.
psnr() {
  ffmpeg -nostdin -v error -i "$1" -i "$2" -lavfi "[0:v][1:v]psnr=stats_file=$OUT/psnr-$3.txt" -f null - || { echo "psnr failed"; return; }
  awk -v w="$WINDOWS" '{for(i=1;i<=NF;i++){split($i,kv,":"); if(kv[1]=="n")n=kv[2]; if(kv[1]=="psnr_avg")p=(kv[2]=="inf"?99:kv[2])}
    f[n-1]=p; s+=p; c++; if(c==1||p<m){m=p; at=n-1}}
    END{printf "mean %.2f dB, min %.2f at frame %d", s/c, m, at; k=split(w,ws,",");
      for(j=1;j<=k;j++){split(ws[j],r,"-"); t=0; q=0; for(x=r[1];x<=r[2];x++) if(x in f){t+=f[x]; q++}; if(q) printf "; f%s: %.2f", ws[j], t/q}}' "$OUT/psnr-$3.txt"
}
strips() {  # strips <cell> <mp4>: each window as one tiled PNG in MEDIA
  local w a b n
  for w in ${WINDOWS//,/ }; do a=${w%-*} b=${w#*-} n=$((b - a + 1))
    ffmpeg -nostdin -v error -y -i "$2" -vf "select='between(n\,$a\,$b)',scale=448:-1,tile=${n}x1" -frames:v 1 "$MEDIA/$1-f$w.png" || say "strip $1 f$w failed"
  done
}
quote "$(param traffic_gb 150)" "$(param need_s 3600)"
rent

cozy machine show "$RID" --json > "$OUT/show-cohort.json" 2>&1
jq -e --argjson c "$COHORT" '.status.runtime==$c.runtime and .status.tensorfs==$c.tensorfs and .status.version==$c.tensord
  and .rental.base_worker_image_digest==$c.image' "$OUT/show-cohort.json" >/dev/null ||
  fail "$NAME runs $(jq -c '{v: .status.version, rt: .status.runtime, tfs: .status.tensorfs, image: .rental.base_worker_image_digest}' "$OUT/show-cohort.json"), not the cohort"
grep -q "^GPU $((GPUS - 1)) " "$OUT/show.txt" || fail "$NAME does not list GPU $((GPUS - 1))"
say "ASSERT $NAME runs the cohort $(jq -c . <<<"$COHORT") on $GPUS GPUs"

for k in "${names[@]}"; do
  pin=$(jq -r --arg k "$k" '.[] | select(.name==$k) | .pin' <<<"$CELLS")
  step "$k" run "$TARGET" --input "$REQ" "${args[@]}" --lane "$LANE" --gpus "$GPUS" ${pin:+--attention-kernel "$pin"} \
    --on "$NAME" --out "$MEDIA/$k"; c=$?
  N=$(runno "$k")
  [ -n "$N" ] || fail "$k named no run (exit $c)"
  cozy run show "$N" --once --json > "$OUT/$k.json" 2>/dev/null; cozy run show "$N" --once > "$OUT/$k.show.txt" 2>&1
  facts "$k" > "$OUT/$k.facts.json" || fail "$k: no record"
  say "$k run $N (exit $c): $(cat "$OUT/$k.facts.json")"
  [ "$(jq -r .status "$OUT/$k.facts.json")" = completed ] || fail "$k did not complete"
  case "$pin" in
    *=sol-attn) jq -e '.route[0] | startswith("sol-attn[sdpa")' "$OUT/$k.facts.json" >/dev/null || fail "$k did not serve Sol with SDPA dense rows" ;;
    *=sdpa) jq -e '.route[0] | startswith("sdpa[")' "$OUT/$k.facts.json" >/dev/null || fail "$k did not serve SDPA" ;;
  esac
  v=$(video "$k"); [ -s "$v" ] || fail "$k: no video"
  strips "$k" "$v"
done

# Same request in every cell: prompt, references, sizing, schedule, cohort and models; else nothing compares.
first=${names[0]}
for k in "${names[@]:1}"; do
  jq -e --slurpfile a "$OUT/$first.facts.json" '[.prompt, .references, .sizing, .schedule, .software, .models, .degree]
    == ($a[0] | [.prompt, .references, .sizing, .schedule, .software, .models, .degree])' "$OUT/$k.facts.json" >/dev/null ||
    fail "$k and $first did not run the same request"
done
p=$(jq -r .prompt "$OUT/$first.facts.json") want=$(jq -r '.prompt // empty' <<<"$EXPECT")
if [ -n "$want" ] && [[ $want == "$p"* || $p == "$want"* ]] && [ "$(jq -c '.references' "$OUT/$first.facts.json")" = "$(jq -c '.references // []' <<<"$EXPECT")" ]; then
  say "ASSERT every cell compiled the recorded prompt ($p) over the recorded references"
else
  say "WARN the cells' prompt $p / references $(jq -c .references "$OUT/$first.facts.json") differ from the recorded ones: the A/B holds, the recorded video is not this request"
fi
say "R0 served $(jq -c .route "$OUT/$first.facts.json") (recorded: $(jq -c '.dense // []' <<<"$EXPECT"))"
[ -z "$rec" ] || { say "$first vs recorded: $(psnr "$(video "$first")" "$rec" "$first-recorded")"; strips recorded "$rec"; }
for ((i = 0; i < ${#names[@]}; i++)); do for ((j = i + 1; j < ${#names[@]}; j++)); do
  a=${names[$i]} b=${names[$j]}; say "$a vs $b: $(psnr "$(video "$a")" "$(video "$b")" "$a-$b")"
done; done
say "strips and videos: $MEDIA (review the windows ${WINDOWS} for translucent figures in each cell)"
