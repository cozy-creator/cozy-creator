# upstream-parity (se-158): anima, sdxl and moss-image from a packages_refs checkout on one rental, each against its
# model's upstream reference at the same seed, noise, prompt, size, steps and guidance: Diffusers' SDXL pipelines and
# Anima modular pipeline, MOSS-Image-SFT's own pipeline_moss_image.py (fixtures/upstream-parity/ref.py). The references
# run first, over SSH in venvs of their own, while the card is idle; then ours through `cozy run`, at --lane bf16
# --attention-kernel exact and at their defaults. The rental ends, and compare.py scores each pair on this runner
# (PSNR, DISTS) with side-by-side crops. Paul 11-17: "make sure we don't have any other errors / deviations like that".
# A dev home on this runner's RC cozy (pod-session `cozy`: an rc-dev build of rc/<x>), as qwen-cutout: the rental moves
# to the RC's builds and is held there. Without `rent`, $0: the scripts compiled, compare.py proven on two fixtures, the
# quote, nothing rented.
# params:
#   tree     the packages_refs checkout (default pk)
#   seed     every cell's seed (default 1234)
#   models   which of sdxl, anima, moss (default all three, space-separated)
#   seconds  what the cap must buy (default 3600)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/upstream-parity IMGS=$HERE/fixtures/ltx-images WORK=${RUNNER_TEMP:-$OUT/..}/upstream-parity
TREE=$(param tree pk) SEED=$(param seed 1234) MODELS=" $(param models 'sdxl anima moss') " NEED=$(param seconds 3600)
SRC=$PACKAGES/$TREE
for tool in jq uv; do command -v "$tool" > /dev/null || fail "no $tool"; done
[[ $SEED =~ ^[0-9]+$ ]] || fail "seed $SEED"
for p in sdxl anima moss-image; do [ -f "$SRC/$p/pyproject.toml" ] || fail "no $p in packages_refs checkout $TREE"; done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
mkdir -p "$WORK" "$OUT/records" "$OUT/pics" "$OUT/ours"
say "tree $TREE: $(git -C "$SRC" rev-parse --short HEAD); $(for p in sdxl anima moss-image; do printf '%s %s ' "$p" "$(sed -n 's/^version = "\(.*\)"/\1/p' "$SRC/$p/pyproject.toml")"; done)"

# The prompts, and the settings both sides share (each package's defaults where upstream has the same).
SP='a red fox sitting in fresh snow at golden hour, detailed fur, photograph'
AP='masterpiece, best quality, 1girl, silver hair, rain, neon city at night'
AN='worst quality, low quality, score_1, score_2, score_3, artist name, blurry, jpeg artifacts, chromatic aberration'
AIP='masterpiece, best quality, a porcelain teapot on a wooden table, anime style illustration'
MP='A sticky note on a fridge, handwritten: "Buy milk and eggs"'
PEARL=$IMGS/pearl.jpg TEAPOT=$IMGS/teapot.jpg
# sdxl `upscale` of pearl.jpg (648x768) at 2 MP: _SIZES.fit on the 64 px grid.
UP_W=1344 UP_H=1600

# The references, one pod command each (refs.sh <name>): ra is Diffusers 0.40.0 with Transformers 5.16.1, as sdxl's
# and anima's locks; rm Diffusers 0.42.0 with Transformers 5.20.2, as moss-image's.
REFS=() REFSH=$'set -e\ncd /root/up\nexport HF_HUB_DISABLE_PROGRESS_BARS=1 TOKENIZERS_PARALLELISM=false\ncase $1 in'
ref() {
  local name=$1 venv=$2; shift 2
  REFS+=("$name"); REFSH+=$'\n'"  $name) exec $venv/bin/python ref.py $(printf '%q ' "$@")--out out/$name.png ;;"
}
if [[ $MODELS == *" sdxl "* ]]; then
  ref ref-sdxl-zeros ra sdxl-t2i --zeros --prompt "$SP" --seed "$SEED" --steps 30 --guidance 7
  ref ref-sdxl-empty ra sdxl-t2i --negative '' --prompt "$SP" --seed "$SEED" --steps 30 --guidance 7
  ref ref-sdxl-empty-math ra sdxl-t2i --negative '' --prompt "$SP" --seed "$SEED" --steps 30 --guidance 7 --attn math
  ref ref-sdxl-up-mode ra sdxl-i2i --image pearl.jpg --width "$UP_W" --height "$UP_H" --strength 0.2 --seed "$SEED" --steps 30 --guidance 7 --encode mode
  ref ref-sdxl-up-sample ra sdxl-i2i --image pearl.jpg --width "$UP_W" --height "$UP_H" --strength 0.2 --seed "$SEED" --steps 30 --guidance 7 --encode sample
fi
if [[ $MODELS == *" anima "* ]]; then
  ref ref-anima ra anima-t2i --prompt "$AP" --negative "$AN" --seed "$SEED" --steps 30 --guidance 4.5
  ref ref-anima-math ra anima-t2i --prompt "$AP" --negative "$AN" --seed "$SEED" --steps 30 --guidance 4.5 --attn math
  ref ref-anima-i2i ra anima-i2i --image teapot.jpg --prompt "$AIP" --negative "$AN" --strength 0.9 --seed "$SEED" --steps 30 --guidance 4.5
fi
if [[ $MODELS == *" moss "* ]]; then
  ref ref-moss rm moss-t2i --prompt "$MP" --negative '' --seed "$SEED" --steps 50 --guidance 3
  ref ref-moss-math rm moss-t2i --prompt "$MP" --negative '' --seed "$SEED" --steps 50 --guidance 3 --attn math
fi
REFSH+=$'\n  *) echo "no reference $1"; exit 2 ;;\nesac'

# Ours: name, then `cozy run` words.
OURS=()
EXACT=(--lane bf16 --attention-kernel exact)
ours() { local name=$1; shift; OURS+=("$name"); eval "OURS_$((${#OURS[@]} - 1))=(\"\$@\")"; }
FULL=(cfg_interval_start:=0 cfg_interval_stop:=1)
if [[ $MODELS == *" sdxl "* ]]; then
  ours ours-sdxl-exact "$SRC/sdxl/generate" "prompt=$SP" "seed:=$SEED" steps:=30 guidance:=7 "${EXACT[@]}"
  ours ours-sdxl-default "$SRC/sdxl/generate" "prompt=$SP" "seed:=$SEED" steps:=30 guidance:=7
  ours ours-sdxl-up-exact "$SRC/sdxl/upscale" --file "image=$PEARL" megapixels:=2 denoise:=0.2 "seed:=$SEED" "${EXACT[@]}"
  ours ours-sdxl-up-default "$SRC/sdxl/upscale" --file "image=$PEARL" megapixels:=2 denoise:=0.2 "seed:=$SEED"
fi
if [[ $MODELS == *" anima "* ]]; then
  ours ours-anima-exact "$SRC/anima/generate" "prompt=$AP" "seed:=$SEED" megapixels:=1 steps:=30 guidance:=4.5 "${FULL[@]}" "${EXACT[@]}"
  ours ours-anima-exact-interval "$SRC/anima/generate" "prompt=$AP" "seed:=$SEED" megapixels:=1 steps:=30 guidance:=4.5 "${EXACT[@]}"
  ours ours-anima-default "$SRC/anima/generate" "prompt=$AP" "seed:=$SEED" megapixels:=1 steps:=30 guidance:=4.5
  ours ours-anima-i2i-exact "$SRC/anima/generate" "prompt=$AIP" --file "image=$TEAPOT" strength:=0.9 "seed:=$SEED" megapixels:=1 steps:=30 guidance:=4.5 "${FULL[@]}" "${EXACT[@]}"
fi
if [[ $MODELS == *" moss "* ]]; then
  ours ours-moss-exact "$SRC/moss-image/generate" "prompt=$MP" "seed:=$SEED" "${EXACT[@]}"
  ours ours-moss-default "$SRC/moss-image/generate" "prompt=$MP" "seed:=$SEED"
fi
[ "${#REFS[@]}" -gt 0 ] || fail "models: none of sdxl, anima, moss in '$MODELS'"

# The pairs compare.py scores: name, a, b, what it measures.
: > "$WORK/pairs.tsv"
pair() { printf '%s\t%s\t%s\t%s\n' "$1" "$OUT/pics/$2" "$OUT/pics/$3" "$4" >> "$WORK/pairs.tsv"; }
if [[ $MODELS == *" sdxl "* ]]; then
  pair sdxl-impl ours-sdxl-exact.png ref-sdxl-empty.png "ours bf16 exact vs Diffusers, both encoding the empty negative"
  pair sdxl-floor ref-sdxl-empty.png ref-sdxl-empty-math.png "Diffusers vs itself on math SDPA: the rounding floor"
  pair sdxl-negative ref-sdxl-empty.png ref-sdxl-zeros.png "Diffusers: CLIP of the empty negative vs its default zeros"
  pair sdxl-upstream ours-sdxl-exact.png ref-sdxl-zeros.png "ours bf16 exact vs Diffusers' default (zeroed negative)"
  pair sdxl-lane ours-sdxl-default.png ours-sdxl-exact.png "ours default lane vs bf16 exact"
  pair sdxl-up-impl ours-sdxl-up-exact.png ref-sdxl-up-mode.png "upscale: ours bf16 exact vs Diffusers img2img (mode latent)"
  pair sdxl-up-encode ref-sdxl-up-mode.png ref-sdxl-up-sample.png "Diffusers img2img: the latent's mode vs its default sample"
  pair sdxl-up-lane ours-sdxl-up-default.png ours-sdxl-up-exact.png "upscale: ours default lane vs bf16 exact"
fi
if [[ $MODELS == *" anima "* ]]; then
  pair anima-impl ours-anima-exact.png ref-anima.png "ours bf16 exact, full-range CFG vs Diffusers"
  pair anima-floor ref-anima.png ref-anima-math.png "Diffusers vs itself on math SDPA: the rounding floor"
  pair anima-interval ours-anima-exact-interval.png ours-anima-exact.png "ours: CFG interval 0.15-0.70 vs full-range CFG"
  pair anima-lane ours-anima-default.png ours-anima-exact-interval.png "ours default lane vs bf16 exact (both the interval)"
  pair anima-upstream ours-anima-default.png ref-anima.png "ours at its defaults vs Diffusers"
  pair anima-i2i-impl ours-anima-i2i-exact.png ref-anima-i2i.png "image to image: ours bf16 exact vs Diffusers"
fi
if [[ $MODELS == *" moss "* ]]; then
  pair moss-impl ours-moss-exact.png ref-moss.png "ours bf16 exact vs upstream's pipeline"
  pair moss-floor ref-moss.png ref-moss-math.png "upstream vs itself on math SDPA: the rounding floor"
  pair moss-lane ours-moss-default.png ours-moss-exact.png "ours default lane (int8, 8-bit attention) vs bf16 exact"
fi

# This runner's scorer, proven on two fixtures before anything is rented.
uv venv -q --python 3.12 "$WORK/py" > "$OUT/py.log" 2>&1 &&
  uv pip install -q --python "$WORK/py/bin/python" --torch-backend cpu pyiqa==0.1.16 'setuptools<81' pillow numpy >> "$OUT/py.log" 2>&1 ||
  fail "the scorer's Python: $(tail -c 400 "$OUT/py.log")"
PY=$WORK/py/bin/python
"$PY" -m py_compile "$FIX/ref.py" "$FIX/compare.py" || fail "ref.py or compare.py does not compile"
"$PY" -I -c 'import sys; from PIL import Image; [Image.open(p).convert("RGB").resize((512, 512)).save(o) for p, o in zip(sys.argv[1:3], sys.argv[3:5])]' \
  "$PEARL" "$TEAPOT" "$WORK/a.png" "$WORK/b.png" || fail "fixtures"
printf 'self\t%s\t%s\tself-check\n' "$WORK/a.png" "$WORK/b.png" > "$WORK/self.tsv"
"$PY" -I "$FIX/compare.py" "$WORK/self.tsv" --out "$WORK/self-scores.tsv" --crops "$WORK/self-crops" > "$OUT/compare-check.txt" 2>&1 ||
  fail "compare.py: $(tail -c 400 "$OUT/compare-check.txt")"
say "scorer: $(tail -1 "$OUT/compare-check.txt")"
printf '%s\n' "$REFSH" > "$WORK/refs.sh"
bash -n "$WORK/refs.sh" || fail "refs.sh does not parse"
for p in sdxl anima moss-image; do kernels "$SRC/$p"; done

# A dev home: the rental moves to the RC's builds.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
run_project "$RUN_PROJECT" || fail "project at $RUN_PROJECT"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
trap 'code=$?; claim; (exit "$code"); finish' EXIT
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh

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
pod "mkdir -p /root/up/out" < /dev/null || fail "mkdir on $NAME"
for f in "$FIX/ref.py" "$WORK/refs.sh" "$PEARL" "$TEAPOT"; do
  pod "cat > /root/up/${f##*/}" < "$f" || fail "copy ${f##*/} to $NAME"
done

# Our side's weights download while the pod sets up the references (no GPU is touched by either).
(
  for m in stabilityai/sdxl@1.0.0:sdxl circlestone-labs/anima@1.0.0:anima openmoss-team/moss-image@0.1.1:moss; do
    [[ $MODELS == *" ${m##*:} "* ]] || continue
    for lane in bf16 int8; do step "install-${m##*:}-$lane" model install "${m%:*}" --lane "$lane" --on "$NAME" -y < /dev/null; done
  done
) & INSTALLS=$!
# The references' venvs (the image's PyTorch when its Python has one, else PyTorch's build for the driver) and checkpoints.
SETUP=$(cat <<'EOF'
set -e
cd /root/up
export PATH=$HOME/.local/bin:$PATH HF_HUB_DISABLE_PROGRESS_BARS=1
command -v uv > /dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh > /dev/null
PY=$(command -v python3)
SYS=--system-site-packages
"$PY" -c 'import torch; assert torch.cuda.is_available()' 2> /dev/null || SYS=''
echo "python $("$PY" --version 2>&1), torch from ${SYS:+the image}${SYS:-the PyTorch index (uv --torch-backend auto)}"
for v in "ra diffusers==0.40.0 transformers==5.16.1 tokenizers==0.23.1" "rm diffusers==0.42.0 transformers==5.20.2 tokenizers==0.23.3"; do
  set -- $v
  uv venv -q --python "$PY" $SYS "$1"
  [ -n "$SYS" ] || uv pip install -q --python "$1/bin/python" --torch-backend auto torch
  uv pip install -q --python "$1/bin/python" "$2" "$3" "$4" huggingface_hub hf_xet safetensors pillow numpy protobuf sentencepiece psutil pyyaml packaging
  uv pip install -q --python "$1/bin/python" --no-deps accelerate==1.15.0
  "$1/bin/python" -c 'import torch, diffusers, transformers; print(torch.__version__, torch.version.cuda, diffusers.__version__, transformers.__version__, torch.cuda.get_device_name())'
done
ra/bin/python ref.py fetch
EOF
)
T=2700 pod "cat > /root/up/setup.sh && bash /root/up/setup.sh" <<<"$SETUP" > "$OUT/pod-setup.txt" 2>&1 || fail "the references' setup: $(tail -c 600 "$OUT/pod-setup.txt")"
sed 's/^/    /' "$OUT/pod-setup.txt" | tail -12 | tee -a "$R"
T=600 pod "cd /root/up && ra/bin/python ref.py tokens-a && rm/bin/python ref.py tokens-m" < /dev/null > "$OUT/tokens.jsonl" 2>&1 || say "tokens: $(tail -c 300 "$OUT/tokens.jsonl")"
say "tokenizers: $(grep -c '"equal": true' "$OUT/tokens.jsonl") equal, $(grep -c '"equal": false' "$OUT/tokens.jsonl") different"
grep '"equal": false' "$OUT/tokens.jsonl" | head -5 | cut -c1-400 | sed 's/^/    /' | tee -a "$R"

# The references, the card idle: nothing of ours has run on it.
bad=0
for r in "${REFS[@]}"; do
  T=1200 pod "bash /root/up/refs.sh $r" < /dev/null > "$OUT/$r.txt" 2>&1 || { say "FAIL $r: $(tail -c 500 "$OUT/$r.txt" | tr '\n' ' ')"; bad=1; continue; }
  say "$r: $(grep '"cell"' "$OUT/$r.txt" | tail -1)"
  T=300 pod "cat /root/up/out/$r.png" < /dev/null > "$OUT/pics/$r.png" || { say "FAIL $r: no picture back"; bad=1; }
done
wait "$INSTALLS" || say "a weights install failed: $(tail -c 300 "$OUT"/install-*.txt | tr '\n' ' ')"

# Ours, through the CLI.
for i in "${!OURS[@]}"; do
  name=${OURS[$i]}; eval "words=(\"\${OURS_$i[@]}\")"
  mkdir -p "$OUT/ours/$name"
  step "$name" run "${words[@]}" --on "$NAME" -y -o "$OUT/ours/$name"
  code=$? n=$(runno "$name")
  [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/records/$name.json" 2> /dev/null
  made=$(find "$OUT/ours/$name" -type f \( -name '*.webp' -o -name '*.png' \) 2> /dev/null | head -1)
  if [ "$code" != 0 ] || [ -z "$made" ]; then say "FAIL $name: run ${n:-none}, exit $code: $(tail -c 300 "$OUT/$name.txt" | tr '\n' ' ')"; bad=1; continue; fi
  "$PY" -I -c 'import sys; from PIL import Image; Image.open(sys.argv[1]).convert("RGB").save(sys.argv[2])' "$made" "$OUT/pics/$name.png"
  say "$name: run $n $(jq -r '"\(.status), exec \((.execution_ms // 0) / 1000) s, peak reserved \((.measurements.memory.peak_reserved_bytes // 0) / 1073741824 * 10 | floor / 10) GiB, kernels \([.. | objects | select(has("kernel") and has("source")) | "\(.kernel)=\(.source)"] | unique | join(" "))"' "$OUT/records/$name.json" 2> /dev/null)"
done
cozy machine show "$NAME" --json > "$OUT/show-end.json" 2>&1
cozy machine logs "$NAME" --since 4h -n 0 > "$OUT/machine.log" 2>&1
# The rental ends before the scoring, which needs only this runner.
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1; done
for r in "${RIDS[@]}"; do for _ in $(seq 60); do gone "$r" && break; sleep 10; done; say "$r: $(gone "$r" && echo gone || echo 'still listed')"; done

cp "$WORK/pairs.tsv" "$OUT/pairs.tsv"
"$PY" -I "$FIX/compare.py" "$WORK/pairs.tsv" --out "$OUT/scores.tsv" --crops "$OUT/crops" > "$OUT/compare.txt" 2>&1 || RC=1
sed 's/^/  /' "$OUT/compare.txt" | tee -a "$R"
{ echo "## upstream-parity on $NAME ($SKU), seed $SEED"; echo '```'; cat "$OUT/compare.txt"; echo '```'; } > "$OUT/summary.md"
cat "$OUT/summary.md" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
[ "$bad" = 0 ] || RC=1
exit "$RC"
