# upstream-parity (se-159): each package run through `cozy run` from packages_refs checkouts (bf16 lane, SDPA pinned)
# beside its upstream reference on the same rental, over SSH (fixtures/upstream-parity/upstream.py: Diffusers' Qwen-Image
# and PAN2 pipelines, Transformers' generate, BiRefNet's model card and handler), the same seed, prompt and size, then
# judged on this runner (compare.py: PSNR and DISTS, frames against the codec's ceiling, text, a known cutout).
# The references run first, each on an idle GPU; the package runs after them.
# A dev home on this runner's RC cozy (pod-session `cozy`: a dev-build or rc-dev run of rc/<x>), held on its builds.
# params:
#   tree     the packages_refs checkout under test (pk)
#   fixed    {package: checkout}: a fix's checkout whose cells run too, judged beside the tree's (default none)
#   groups   which of qwen-image, pan2, qwen3, birefnet, rewriter (comma-separated; default all)
#   seconds  what the cap must buy (default 10800)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/upstream-parity WORK=${RUNNER_TEMP:-$OUT/..}/upstream-parity
TREE=$(param tree pk) FIXED=$(param fixed '{}') NEED=$(param seconds 10800)
GROUPS_=",$(param groups 'qwen-image,pan2,qwen3,birefnet,rewriter'),"
on() { [[ $GROUPS_ == *",$1,"* ]]; }
for tool in jq uv ffmpeg ffprobe curl; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -d "$PACKAGES/$TREE" ] || fail "no packages_refs checkout $TREE"
jq -e 'type == "object"' <<<"$FIXED" > /dev/null || fail "fixed: a JSON object {package: checkout}"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
mkdir -p "$WORK/fx" "$WORK/ours" "$WORK/up" "$OUT/compare" "$OUT/inputs"
# The pictures: release-gate's public fixtures, and PAN2's own examples at the reference's commit.
cp "$HERE/fixtures/ltx-images/teapot.jpg" "$HERE/fixtures/ltx-images/pearl.jpg" "$WORK/fx/" || fail "fixtures"
PAN2_SRC=https://raw.githubusercontent.com/ifm-ai/PAN2/0fb907b6e16a82ead6d691d1de2eef6299c71e9a/inference/prompts
curl -fsSL --retry 5 -o "$WORK/fx/pan2-examples.json" "$PAN2_SRC/examples.json" &&
  curl -fsSL --retry 5 -o "$WORK/fx/i2v_house_tour.jpg" "$PAN2_SRC/i2v_house_tour.jpg" || fail "PAN2's examples"
cp "$FIX/cells.json" "$WORK/fx/"
CELLS=$FIX/cells.json
# Each checkout's packages, their Triton sets built first.
trees() { echo "$TREE"; jq -r '.[]' <<<"$FIXED"; }
for t in $(trees | sort -u); do
  [ -d "$PACKAGES/$t" ] || fail "no packages_refs checkout $t"
  for p in qwen-image pan2 qwen3 birefnet; do [ ! -f "$PACKAGES/$t/$p/pyproject.toml" ] || kernels "$PACKAGES/$t/$p"; done
  say "tree $t: $(git -C "$PACKAGES/$t" rev-parse --short HEAD)"
done
# The judge's Python, proven before anything is rented.
uv venv -q --python 3.12 "$WORK/py" > "$OUT/py.log" 2>&1 &&
  uv pip install -q --python "$WORK/py/bin/python" --index-url https://download.pytorch.org/whl/cpu torch torchvision >> "$OUT/py.log" 2>&1 &&
  uv pip install -q --python "$WORK/py/bin/python" numpy pillow piq >> "$OUT/py.log" 2>&1 || fail "the judge's Python: $(tail -c 400 "$OUT/py.log")"
PY=$WORK/py/bin/python
"$PY" -I "$FIX/compare.py" --self-check "$WORK/fx/teapot.jpg" > "$OUT/judge-check.txt" 2>&1 || fail "compare.py: $(tail -c 400 "$OUT/judge-check.txt")"
say "$(cat "$OUT/judge-check.txt")"

# A dev home: the rental moves to the RC's builds.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
run_project "$RUN_PROJECT" || fail "project at $RUN_PROJECT"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
trap 'code=$?; claim; (exit "$code"); finish' EXIT

quote "$NEED"
order
ready
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
[ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
say "on the RC: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · " + ([.status.gpus[] | "GPU \(.index) \(.name) \(.memory_bytes / 1073741824 | floor) GiB"] | join(", "))' "$OUT/show-rc.json")"
for try in 1 2 3; do
  step "hold-$try" machine update "$NAME" --runtime "$(jq -r '.status.runtime' "$OUT/show-rc.json")" -y
  for _ in $(seq 180); do
    cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
    [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && { [ -n "$(jq -r '.held // empty' "$OUT/show-rc.json")" ] || [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ]; } && break
    sleep 5
  done
  [ -z "$(jq -r '.held // empty' "$OUT/show-rc.json")" ] || break
done
[ -n "$(jq -r '.held // empty' "$OUT/show-rc.json")" ] || fail "$NAME isn't held on the RC's builds: $(tail -c 300 "$OUT/hold-$try.txt")"
say "held: $(jq -r '.held' "$OUT/show-rc.json")"

pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
say "pod: $(pod 'nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader; free -g | sed -n 2p; df -h / | tail -1; nproc' < /dev/null | paste -sd'|')"
pod "mkdir -p /root/up/fx /root/up/out" < /dev/null || fail "the pod's work folder"
for f in upstream.py setup.sh cells.json; do pod "cat > /root/up/$f" < "$FIX/$f" || fail "copy $f"; done
for f in "$WORK"/fx/*; do pod "cat > /root/up/fx/${f##*/}" < "$f" || fail "copy ${f##*/}"; done
pod "cd /root/up && setsid nohup bash setup.sh > setup.log 2>&1 < /dev/null &" < /dev/null || fail "setup.sh"
say "the references' environments and weights: fetching on the pod"

# Our weights meanwhile, each lane the cells pin (bf16), on the pod's TensorFS.
QI=qwen/qwen-image-2512@sha256:447b8c6848773b8730ff4cf540d0fd77bcf68d1d57d06a990c7f8b0aad35a1c4
QE=qwen/qwen-image-edit-2511@sha256:f8c99413497ff7c869060d25190b6f254d16939e24997c0e6e6b91eca95be292
P2=ifm/pan2@sha256:649bd825e2df81c918cb47887562dd21a0c3b4d835ab3146a4126d1f2247c689
Q3=huihui-ai/huihui-qwen3.5-9b-abliterated@sha256:3d23b0ddeb4c310b7ee8c374dc365334d9a025a8fa32507041fc9d03f94fa0b8
installs=()
on qwen-image && installs+=("$QI" "$QE" lightx2v/qwen-image-2512-lightning-lora@0.1.0 lightx2v/qwen-image-edit-2511-lightning-lora@0.1.0)
on birefnet && installs+=(zhengpeng7/birefnet@0.1.0)
on qwen3 && installs+=("$Q3")
on pan2 && installs+=("$P2")
( for i in "${!installs[@]}"; do step "install-$i" model install "${installs[$i]}" --on "$NAME" -y < /dev/null || say "install ${installs[$i]}: failed"; done ) & BG=$!

# The references, one group at a time on the idle GPU, as its weights arrive; the pod kept from its idle end.
alive=$(date +%s)
keep() { [ $(($(date +%s) - alive)) -lt 300 ] || { cozy machine keepalive "$NAME" > /dev/null 2>&1; alive=$(date +%s); }; }
waitfor() {  # waitfor <file> <seconds>: true once the pod holds /root/up/<file>, false at a .failed or the time
  local end=$(($(date +%s) + $2)) base=${1%.*}
  while [ "$(date +%s)" -lt "$end" ]; do
    keep
    case $(pod "cd /root/up; [ -f '$1' ] && echo yes; [ -f '$base.failed' ] && echo failed" < /dev/null 2> /dev/null) in
      *yes*) return 0 ;; *failed*) return 1 ;;
    esac
    sleep 30
  done
  return 1
}
envs_for() { case $1 in qwen-image | qwen3) echo a ;; *) echo b ;; esac; }
for g in qwen-image birefnet qwen3 pan2; do
  on "$g" || continue
  if ! waitfor "$g.fetched" 5400; then say "reference $g: its weights never arrived: $(pod "tail -5 /root/up/fetch-$g.log /root/up/setup.log /root/up/$(envs_for "$g").log" < /dev/null 2>&1 | tail -c 600)"; continue; fi
  e=$(envs_for "$g")
  pod "cd /root/up && setsid nohup bash -c 'HF_HOME=/root/hf $e/bin/python upstream.py $g out fx > ref-$g.log 2>&1 || touch out/$g.failed' > /dev/null 2>&1 < /dev/null &" < /dev/null
  if waitfor "out/$g.done" 3600; then say "reference $g: $(pod "cat /root/up/out/$g.done" < /dev/null)"
  else say "reference $g FAILED: $(pod "tail -15 /root/up/ref-$g.log" < /dev/null 2>&1 | tail -c 1500)"; fi
  pod "tail -40 /root/up/ref-$g.log" < /dev/null > "$OUT/ref-$g.log" 2>&1
  # Its weights go: the disk holds the next group's and ours.
  pod "rm -rf /root/hf/hub/models--$(case $g in qwen-image) echo 'Qwen--Qwen-Image-* /root/hf/hub/models--lightx2v--*' ;; pan2) echo IFM--PAN2 ;; qwen3) echo huihui-ai--* ;; *) echo ZhengPeng7--* ;; esac)" < /dev/null
done
T=900 pod "tar -C /root/up/out -cf - ." < /dev/null > "$WORK/up.tar" && tar -xf "$WORK/up.tar" -C "$WORK/up" || fail "the references' outputs"
say "references: $(ls "$WORK/up" | paste -sd' ')"
wait "$BG"; BG=''

# Ours: each cell through `cozy run` from its checkout, bf16, SDPA.
EXACT=(--attention-kernel sdpa --on "$NAME" -y)
: > "$WORK/cells.tsv"
ours() {  # ours <name> <checkout> <package/function> <kind> <reference> [extra] -- <cozy run args…>
  local name=$1 tree=$2 target=$3 kind=$4 ref=$5 extra=$6; shift 7
  mkdir -p "$WORK/ours/$name"
  (cd "$PACKAGES/$tree" && step "$name" run "./$target" "$@" -o "$WORK/ours/$name") < /dev/null
  local code=$? n; n=$(runno "$name")
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$name.json" 2> /dev/null
  local made
  case $kind in
    text | rewrite) made=$OUT/$name.result.json; jq '{texts: [(.result.texts[0] // .result.prompt)]}' "$OUT/$name.json" > "$made" 2> /dev/null ;;
    video) made=$(find "$WORK/ours/$name" -type f -name '*.mp4' | head -1) ;;
    *) made=$(find "$WORK/ours/$name" -type f \( -name '*.png' -o -name '*.webp' \) | head -1) ;;
  esac
  say "$name: run ${n:-none} exit $code: $(jq -r '"\(.status), exec \((.execution_ms // 0) / 1000) s, peak reserved \((.measurements.memory.peak_reserved_bytes // 0) / 1073741824 * 10 | floor / 10) GiB, kernels \([.. | objects | select(has("kernel") and has("source")) | .kernel] | unique | join(" "))"' "$OUT/$name.json" 2> /dev/null)"
  [ "$code" = 0 ] && [ -s "$made" ] || { say "FAIL $name: $(tail -c 300 "$OUT/$name.txt" | tr '\n' ' ')"; return 1; }
  printf '%s\t%s\t%s\t%s\t%s\n' "$name" "$kind" "$made" "$ref" "$extra" >> "$WORK/cells.tsv"
}
cell() { jq -r --arg g "$1" --arg c "$2" --arg k "$3" '.[$g][$c][$k]' "$CELLS"; }
fixed() { jq -r --arg p "$1" '.[$p] // empty' <<<"$FIXED"; }
for g in qwen-image pan2 qwen3 birefnet rewriter; do
  on "$g" || continue
  case $g in
    qwen-image)
      prompt=$(cell qwen-image t2i prompt)
      ours qwen-image-t2i "$TREE" qwen-image/generate image qwen-image-t2i.png '' -- "prompt=$prompt" seed:=42 --lane bf16 "${EXACT[@]}"
      ours qwen-image-t2i-turbo "$TREE" qwen-image/generate_turbo image qwen-image-t2i-turbo.png '' -- "prompt=$prompt" seed:=42 --lane bf16 "${EXACT[@]}"
      edit=$(cell qwen-image edit prompt) edit=${edit/Picture 1/<teapot>}
      refs="references:=[{\"name\":\"teapot\",\"image\":\"$WORK/fx/teapot.jpg\"}]"
      ours qwen-image-edit "$TREE" qwen-image/generate image qwen-image-edit.png '' -- "prompt=$edit" "$refs" seed:=42 --lane bf16 --model "edit_model=$QE" "${EXACT[@]}"
      ours qwen-image-edit-turbo "$TREE" qwen-image/generate_turbo image qwen-image-edit-turbo.png '' -- "prompt=$edit" "$refs" seed:=42 --lane bf16 --model "edit_model=$QE" "${EXACT[@]}" ;;
    pan2)
      for c in t2v i2v; do
        jq --argjson e "$(cell pan2 $c example)" '{prompt: .[$e].refined_prompt, prompt_upsampling: false, duration_s: 1, aspect_ratio: "16:9", seed: 42}' \
          "$WORK/fx/pan2-examples.json" > "$OUT/inputs/pan2-$c.json"
        files=(); [ "$c" = t2v ] || files=(--file "first_frame=$WORK/fx/i2v_house_tour.jpg")
        ours "pan2-$c" "$TREE" pan2/generate video "pan2-$c.npy" "pan2-$c-vae32.npy" -- --input "$OUT/inputs/pan2-$c.json" "${files[@]}" --model "model=$P2" "${EXACT[@]}"
      done
      # The rewriter inside pan2, each checkout: its caption's timeline against the clip (one step renders; the caption
      # is the result's `prompt`).
      p=$(jq -r '.rewriter.requests[0].prompt' "$CELLS")
      for t in $(echo "$TREE"; fixed pan2); do
        ours "pan2-rewrite-$t" "$t" pan2/generate rewrite 5 '' -- "prompt=$p" duration_s:=5 steps:=1 seed:=42 --model "model=$P2" --on "$NAME" -y
      done
      # Paul's default attention on the DiT blocks (8-bit), the same bf16 lane.
      ours pan2-t2v-attention8 "$TREE" pan2/generate video pan2-t2v.npy pan2-t2v-vae32.npy -- --input "$OUT/inputs/pan2-t2v.json" --model "model=$P2" --on "$NAME" -y ;;
    qwen3)
      for c in text image; do
        jq -n --arg p "$(cell qwen3 $c prompt)" --argjson m "$(cell qwen3 $c max_tokens)" '{prompt: $p, max_tokens: $m}' > "$OUT/inputs/qwen3-$c.json"
        files=(); [ "$c" = text ] || files=(--file "images=$WORK/fx/teapot.jpg")
        ours "qwen3-$c" "$TREE" qwen3/ask text "$c" '' -- --input "$OUT/inputs/qwen3-$c.json" "${files[@]}" --model "model=$Q3" "${EXACT[@]}"
      done ;;
    birefnet)
      for t in $(echo "$TREE"; fixed birefnet); do
        for picture in pearl teapot; do
          ref=birefnet-$picture-card.png; [ "$t" = "$TREE" ] || ref=birefnet-$picture-handler.png
          ours "birefnet-$picture-$t" "$t" birefnet/remove_background image "$ref" '' -- --file "image=$WORK/fx/$picture.jpg" "${EXACT[@]}"
        done
        [ -f "$WORK/up/birefnet-green.png" ] &&
          ours "birefnet-green-$t" "$t" birefnet/remove_background green birefnet-green-truth.png '' -- --file "image=$WORK/up/birefnet-green.png" "${EXACT[@]}"
      done ;;
    rewriter)
      # PAN2's rewriter, as pan2 asks it: Qwen3.5-9B (its reasoner), the policy as its system turn, greedy, thinking off;
      # the user message as the package writes it (`prose`) and as the policy's input contract states it (`record`).
      policy=$PACKAGES/$TREE/pan2/pan2/prompt-policy.md
      for i in 0 1 2; do
        p=$(jq -r ".rewriter.requests[$i].prompt" "$CELLS") s=$(jq -r ".rewriter.requests[$i].seconds" "$CELLS")
        jq -n --rawfile sys "$policy" --arg u "Clip duration: $s seconds.
User request: $p" '{system: $sys, prompt: $u, max_tokens: 3072, temperature: 0, thinking: false}' > "$OUT/inputs/rewrite-prose-$i.json"
        jq -n --rawfile sys "$policy" --arg u "$(jq -cn --arg p "$p" --argjson s "$s" '{original_prompt: $p, duration_sec: $s}')" \
          '{system: $sys, prompt: $u, max_tokens: 3072, temperature: 0, thinking: false}' > "$OUT/inputs/rewrite-record-$i.json"
        for form in prose record; do
          ours "rewrite-$form-$i" "$TREE" qwen3/ask rewrite "$s" '' -- --input "$OUT/inputs/rewrite-$form-$i.json" --model model=qwen/qwen3.5-9b@0.2.0 --on "$NAME" -y
        done
      done ;;
  esac
  keep
done
cozy machine show "$NAME" --json > "$OUT/show-end.json" 2>&1
cozy machine logs "$NAME" --since 6h -n 0 > "$OUT/machine.log" 2>&1
# The rental ends before the judging, which needs only this runner.
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1; done
for r in "${RIDS[@]}"; do for _ in $(seq 60); do gone "$r" && break; sleep 10; done; say "$r: $(gone "$r" && echo gone || echo 'still listed')"; done

"$PY" -I "$FIX/compare.py" "$WORK/cells.tsv" "$WORK/up" "$OUT/compare" > "$OUT/compare.txt" 2>&1 || RC=1
sed 's/^/  /' "$OUT/compare.txt" | tee -a "$R"
cp "$WORK/cells.tsv" "$OUT/"; cp -r "$WORK/ours" "$OUT/ours"; mkdir -p "$OUT/up"
find "$WORK/up" -maxdepth 1 -type f \( -name '*.png' -o -name '*.json' -o -name '*.done' \) -exec cp {} "$OUT/up/" \;
{ echo "## upstream-parity on $NAME ($SKU)"; echo '```'; cat "$OUT/compare.txt"; echo '```'; } > "$OUT/summary.md"
cat "$OUT/summary.md" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
