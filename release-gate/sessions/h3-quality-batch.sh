# h3-quality-batch: one multi-GPU rental, two H3 questions on recorded requests (prepared references, seeds).
# A, on the recorded cohort: does the SM120 attention route make 19472's translucent figures? R0 serves the default
#   route, R2 pins exact SDPA; R1 (Sol sparse, SDPA dense rows) runs only when the ghost score shows R0's episode and
#   not R2's. Every cell must compile the same prompt over the same references.
# B, on the patched Runtime: does a Turbo call whose DiT pages were displaced bake its LoRA into every refilled block?
#   T1 (first bake), T2a (warm, resident), a base call D on the same GPUs to displace pages, T2b (warm, refilled). T2b
#   must refill from the store, read factors for every refilled LoRA block, and decode the same frames as T2a.
# The requests and media are the owner's: they never enter OUT. On a hosted runner it refuses; from the owner's computer:
#   OUT=… SKU=rtx-pro-6000 DISK=200 CAP=26 RENT=false PARAMS="$(cat params.json)" bash release-gate/sessions/h3-quality-batch.sh
# params:
#   base, turbo  {"target", "request", "assets": [{"field", "path", "sha256"}], "recorded": {"path", "sha256"}}
#   expect       {"prompt", "references": [sha256…]}: what 19472's record compiled
#   lane (int8), gpus (4), cohort {"runtime", "tensorfs", "tensord", "image"}, patch (0.33.3), require_patch (true)
#   python, ghost   the detector (h3-video-forensics' ghost.py, standalone) and an interpreter with numpy/scipy/cv2
#   window (248-273): the episode window the rule reads; windows (14-22,55-65,248-273): strips and scores
#   r1_min (100): the R0 event size that counts as the episode; r1_ratio (0.5): R2's share of it that counts as gone
#   media: directory for videos and strips (outside OUT); traffic_gb (150), need_s (4800), stall_s (1500)
. "$(dirname "$0")/lib.sh"
[ "${GITHUB_ACTIONS:-}" != true ] || fail "private request media: run this session from the owner's computer, not a hosted runner"
LANE=$(param lane int8) GPUS=$(param gpus 4) COHORT=$(param cohort '{}') EXPECT=$(param expect '{}') PATCH=$(param patch 0.33.3)
MEDIA=$(param media) WINDOW=$(param window 248-273) WINDOWS=$(param windows 14-22,55-65,248-273) PY=$(param python) GHOST=$(param ghost)
R1_MIN=$(param r1_min 100) R1_RATIO=$(param r1_ratio 0.5) STALL_S=$(param stall_s 1500)
command -v ffmpeg >/dev/null && command -v sha256sum >/dev/null || fail "no ffmpeg/sha256sum"
[ -n "$MEDIA" ] && [[ $MEDIA != "$OUT"* ]] || fail "media must be a directory outside OUT"
mkdir -p "$MEDIA"
[[ $GPUS =~ ^[1-8]$ && $WINDOWS =~ ^[0-9]+-[0-9]+(,[0-9]+-[0-9]+)*$ && ,$WINDOWS, == *,$WINDOW,* ]] || fail "gpus/windows/window"
[[ $PATCH =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "patch $PATCH is not a release version"
jq -e '.runtime and .tensorfs and .tensord and .image' <<<"$COHORT" >/dev/null || fail "cohort needs runtime, tensorfs, tensord, image"
"$PY" -I -c 'import numpy, scipy, cv2' 2>/dev/null && [ -f "$GHOST" ] || fail "ghost detector: $PY $GHOST"
declare -A TARGET REQ RECORDED
ASSETS_base=() ASSETS_turbo=()
for kind in base turbo; do
  spec=$(param "$kind" '{}')
  TARGET[$kind]=$(jq -r '.target // empty' <<<"$spec") REQ[$kind]=$(jq -r '.request // empty' <<<"$spec")
  [[ ${TARGET[$kind]} =~ ^[a-z0-9-]+/[a-z0-9_]+@[0-9.]+$ ]] || fail "$kind target: ${TARGET[$kind]}"
  jq -e 'type=="object" and has("prompt") and has("seed")' "${REQ[$kind]}" >/dev/null 2>&1 || fail "$kind request needs a prompt and a seed"
  while read -r a; do
    f=$(jq -r .field <<<"$a") p=$(jq -r .path <<<"$a") h=$(jq -r .sha256 <<<"$a")
    [[ $f =~ ^[a-z_]+(\.[0-9a-z_]+)+$ ]] && [ "$(sha256sum "$p" 2>/dev/null | cut -c1-64)" = "$h" ] || fail "$kind asset $f: $p is not sha256:$h"
    eval "ASSETS_$kind+=(--asset \"\$f=\$p\")"
  done < <(jq -c '.assets[]' <<<"$spec")
  RECORDED[$kind]=$(jq -r '.recorded.path // empty' <<<"$spec")
  [ "$(sha256sum "${RECORDED[$kind]}" 2>/dev/null | cut -c1-64)" = "$(jq -r '.recorded.sha256' <<<"$spec")" ] || fail "$kind recorded video"
  sha256sum "${REQ[$kind]}" | cut -c1-64 > "$OUT/$kind-request.sha256.txt"  # the payloads stay out of OUT
done
[ "${#ASSETS_base[@]}" -gt 0 ] && [ "${#ASSETS_turbo[@]}" -gt 0 ] || fail "no assets"
# The patched Runtime must exist where `machine update --runtime` fetches it: PyPI, else its GitHub prerelease.
if curl -fsS -o /dev/null "https://pypi.org/pypi/cozy-runtime/$PATCH/json" 2>/dev/null; then say "patch: cozy-runtime $PATCH on PyPI"
elif gh release view "v$PATCH" -R cozy-creator/cozy-runtime --json assets --jq '.assets[].name' 2>/dev/null | grep -q "cozy_runtime-$PATCH-.*manylinux.*x86_64\.whl"; then
  say "patch: cozy-runtime $PATCH as GitHub prerelease v$PATCH"
elif [ "$(param require_patch true)" = true ]; then fail "cozy-runtime $PATCH is not on PyPI or a GitHub prerelease yet"
else say "WARN patch $PATCH not published: pre-flight continues (require_patch=false); a rental would stop at the update"; fi

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
      denoise_s: (($m.attribution.stages.denoise.total_ms // 0) / 1000 | floor),
      fills: [$m.memory.stats.varena[]? | {gpu, dit: [.stats.trace[] | select(.set | test("/transformer(_ref)?#"))
                | select(.origin != "hit") | {block, origin}], overlay: [.stats.trace[] | select(.set | test("_turbo#"))] | length}]}' "$j"
}
video() { jq -r '.output[]?.path' "$OUT/$1.json" 2>/dev/null | grep -E '\.mp4$' | head -1; }
frames_md5() { ffmpeg -nostdin -v error -i "$1" -map 0:v:0 -f framemd5 - | grep -v '^#' | md5sum | cut -c1-16; }
# psnr <a.mp4> <b.mp4> <tag>: per-frame PSNR into OUT, then "mean min frame-of-min" and each window's mean.
psnr() {
  ffmpeg -nostdin -v error -i "$1" -i "$2" -lavfi "[0:v][1:v]psnr=stats_file=$OUT/psnr-$3.txt" -f null - || { echo "psnr failed"; return; }
  awk -v w="$WINDOWS" '{for(i=1;i<=NF;i++){split($i,kv,":"); if(kv[1]=="n")n=kv[2]; if(kv[1]=="psnr_avg")p=(kv[2]=="inf"?99:kv[2])}
    f[n-1]=p; s+=p; c++; if(c==1||p<m){m=p; at=n-1}}
    END{printf "mean %.2f dB, min %.2f at frame %d", s/c, m, at; k=split(w,ws,",");
      for(j=1;j<=k;j++){split(ws[j],r,"-"); t=0; q=0; for(x=r[1];x<=r[2];x++) if(x in f){t+=f[x]; q++}; if(q) printf "; f%s: %.2f", ws[j], t/q}}' "$OUT/psnr-$3.txt"
}
strips() {  # strips <cell> <mp4>: each window as one tiled PNG in MEDIA
  local w a b
  for w in ${WINDOWS//,/ }; do a=${w%-*} b=${w#*-}
    ffmpeg -nostdin -v error -y -i "$2" -vf "select='between(n\,$a\,$b)',scale=448:-1,tile=$((b - a + 1))x1" -frames:v 1 "$MEDIA/$1-f$w.png" || say "strip $1 f$w failed"
  done
}
ghost() { nice -n 19 "$PY" -I "$GHOST" "$1" "$WINDOWS" > "$OUT/ghost-$2.json" 2>> "$OUT/ghost.err" && cat "$OUT/ghost-$2.json"; }
# cell <name> <base|turbo> [pin]: one run of the recorded request, its record, facts, strips and ghost score.
cell() {
  local k=$1 kind=$2 pin=${3:-} N c v
  local -n assets="ASSETS_$kind"
  step "$k" run "${TARGET[$kind]}" --input "${REQ[$kind]}" "${assets[@]}" --lane "$LANE" --gpus "$GPUS" \
    ${pin:+--attention-kernel "$pin"} --on "$NAME" --out "$MEDIA/$k"; c=$?
  N=$(runno "$k"); [ -n "$N" ] || fail "$k named no run (exit $c)"
  cozy run show "$N" --once --json > "$OUT/$k.json" 2>/dev/null; cozy run show "$N" --once > "$OUT/$k.show.txt" 2>&1
  facts "$k" > "$OUT/$k.facts.json" || fail "$k: no record"
  say "$k run $N (exit $c): $(jq -c 'del(.fills)' "$OUT/$k.facts.json")"
  [ "$(jq -r .status "$OUT/$k.facts.json")" = completed ] || fail "$k did not complete"
  case "$pin" in
    *=sol-attn) jq -e '.route[0] | startswith("sol-attn[sdpa")' "$OUT/$k.facts.json" >/dev/null || fail "$k did not serve Sol with SDPA dense rows" ;;
    *=sdpa) jq -e '.route[0] | startswith("sdpa[")' "$OUT/$k.facts.json" >/dev/null || fail "$k did not serve SDPA" ;;
  esac
  v=$(video "$k"); [ -s "$v" ] || fail "$k: no video"
  strips "$k" "$v"; say "$k ghost: $(ghost "$v" "$k")"
}
same() {  # same <a> <b>: both compiled one request on one cohort
  jq -e --slurpfile a "$OUT/$1.facts.json" '[.prompt, .references, .sizing, .schedule, .software, .models, .degree]
    == ($a[0] | [.prompt, .references, .sizing, .schedule, .software, .models, .degree])' "$OUT/$2.facts.json" >/dev/null || fail "$1 and $2 did not run the same request"
}
score() { jq -r --arg w "$WINDOW" '.windows[$w + "_event"] // 0' "$OUT/ghost-$1.json" 2>/dev/null || echo 0; }
cohort() {  # cohort <runtime>: the machine runs it on the recorded TensorFS, TensorD and image
  cozy machine show "$RID" --json > "$OUT/show-$1.json" 2>&1
  jq -e --argjson c "$COHORT" --arg rt "$1" '.status.runtime==$rt and .status.tensorfs==$c.tensorfs and .status.version==$c.tensord
    and .rental.base_worker_image_digest==$c.image' "$OUT/show-$1.json" >/dev/null ||
    fail "$NAME runs $(jq -c '{v: .status.version, rt: .status.runtime, tfs: .status.tensorfs, image: .rental.base_worker_image_digest}' "$OUT/show-$1.json")"
  say "ASSERT $NAME runs Runtime $1 on $(jq -c '{tensorfs, tensord, image}' <<<"$COHORT"), $GPUS GPUs"
}
say "A: ${TARGET[base]} R0, R2 (transformer_ref=sdpa), R1 (transformer_ref=sol-attn) if R0's f$WINDOW event >= $R1_MIN and R2's < $R1_RATIO of it"
say "B: Runtime $PATCH, ${TARGET[turbo]} T1, T2a, D (${TARGET[base]}), T2b"
quote "$(param traffic_gb 150)" "$(param need_s 4800)"
rent
grep -q "^GPU $((GPUS - 1)) " "$OUT/show.txt" || fail "$NAME does not list GPU $((GPUS - 1))"

# A: the attention route, on the recorded cohort.
cohort "$(jq -r .runtime <<<"$COHORT")"
cell R0 base; cell R2 base transformer_ref=sdpa; same R0 R2
if [ "$(jq -r .prompt "$OUT/R0.facts.json")" = "$(jq -r '.prompt // empty' <<<"$EXPECT")" ] &&
   [ "$(jq -c .references "$OUT/R0.facts.json")" = "$(jq -c '.references // []' <<<"$EXPECT")" ]; then
  say "ASSERT A compiled the recorded prompt over the recorded references"
else say "WARN A's prompt or references differ from the recorded ones: the A/B holds, the recorded video is not this request"; fi
say "R0 served $(jq -c .route "$OUT/R0.facts.json"); R0 vs recorded: $(psnr "$(video R0)" "${RECORDED[base]}" R0-recorded)"
strips recorded-base "${RECORDED[base]}"; say "recorded ghost: $(ghost "${RECORDED[base]}" recorded-base)"
say "R0 vs R2: $(psnr "$(video R0)" "$(video R2)" R0-R2)"
s0=$(score R0) s2=$(score R2)
if [ "$s0" -ge "$R1_MIN" ] && awk -v a="$s2" -v b="$s0" -v r="$R1_RATIO" 'BEGIN{exit !(a < r * b)}'; then
  say "R1: R0's f$WINDOW event is $s0, R2's $s2: the route matters; Sol with exact dense rows tells which half"
  cell R1 base transformer_ref=sol-attn; same R0 R1
  say "R0 vs R1: $(psnr "$(video R0)" "$(video R1)" R0-R1); R1 vs R2: $(psnr "$(video R1)" "$(video R2)" R1-R2)"
else say "R1 skipped: R0's f$WINDOW event $s0, R2's $s2 (needs >= $R1_MIN and R2 < $R1_RATIO of it)"; fi

# B: the LoRA bake on refilled pages, on the patched Runtime.
step update machine update "$NAME" --runtime "$PATCH" -y || fail "machine update to Runtime $PATCH"
cohort "$PATCH"
cell T1 turbo; cell T2a turbo; cell D base; cell T2b turbo; same T1 T2a; same T2a T2b
say "T1 vs recorded turbo: $(psnr "$(video T1)" "${RECORDED[turbo]}" T1-recorded)"
strips recorded-turbo "${RECORDED[turbo]}"; say "recorded turbo ghost: $(ghost "${RECORDED[turbo]}" recorded-turbo)"
say "T1 vs T2a: $(psnr "$(video T1)" "$(video T2a)" T1-T2a); D vs R0 (base on $PATCH vs the recorded cohort): $(psnr "$(video D)" "$(video R0)" D-R0)"
# Per GPU: blocks T2b refilled from the store (block 0 holds no LoRA site) and the overlay reads beyond T2a's.
jq -c --slurpfile a "$OUT/T2a.facts.json" '[.fills[] | . as $g | ($a[0].fills[] | select(.gpu == $g.gpu)) as $w |
  {gpu, refilled: ([$g.dit[] | select(.origin == "store" and .block != 0)] | length), derived: ([$g.dit[] | select(.origin != "store")] | length),
   reads: ($g.overlay - $w.overlay)}]' "$OUT/T2b.facts.json" > "$OUT/T2b-refill.json"
say "T2b refills per GPU: $(cat "$OUT/T2b-refill.json")"
refilled=$(jq '[.[].refilled] | add // 0' "$OUT/T2b-refill.json")
[ "$refilled" -gt 0 ] || fail "D displaced no Turbo DiT page: T2b refilled nothing, the patch is not exercised"
jq -e 'all(.[]; .reads >= 6 * .refilled)' "$OUT/T2b-refill.json" >/dev/null || fail "a refilled LoRA block was not baked (fewer than 6 factor reads per refilled block)"
fa=$(frames_md5 "$(video T2a)") fb=$(frames_md5 "$(video T2b)")
if [ "$fa" = "$fb" ]; then say "ASSERT T2b decodes the same frames as T2a ($fa) after refilling $refilled blocks"
else
  p=$(psnr "$(video T2a)" "$(video T2b)" T2a-T2b); say "T2a vs T2b: frames differ ($fa / $fb): $p"
  awk -v m="$(awk '{print $2}' <<<"$p")" 'BEGIN{exit !(m >= 45)}' || fail "T2b differs from T2a below 45 dB: refilled blocks do not serve T2a's weights"
fi
say "strips and videos: $MEDIA (review f${WINDOWS//,/, f} in every cell and both recorded videos)"
