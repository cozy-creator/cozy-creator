# upstream-parity (se-162): our packages against their model makers' own code, on one rental. Paul, 11-17: "make sure we
# don't have any other errors / deviations like that in our implementations for other packages as well".
# Ours: each cell through `cozy run` from a packages_refs checkout. Upstream: HiDream-ai/HiDream-O1-Image's and
# ideogram-oss/ideogram4's own code (fixtures/upstream-parity/*_ref.py), its environment built here and shipped to the pod, its
# weights downloaded there from Hugging Face. Same prompt, seed and size, bf16, exact attention (SDPA) on both sides. Then,
# on the runner: PSNR, mean |difference| and DISTS of each of ours against upstream's, and crop sheets (compare.py).
# params:
#   tree     the packages_refs checkout (pk)
#   only     cells to run, comma-separated (default all): h-t2i, h-edit, h-dev-edit, h-default, i-fp8, i-default
#   seed_h   HiDream's seed (32, inference.py's default); seed_i Ideogram's (7)
#   need_s   seconds the cap must buy (4200)
#   floor_mb_s  lib.sh's slow-host floor: upstream's weights download beside ours, so give it about half the usual (25)
. "$(dirname "$0")/lib.sh"
FX=$(cd "$(dirname "$0")" && pwd)/fixtures/upstream-parity
TREE=$(param tree pk) ONLY=",$(param only 'h-t2i,h-edit,h-dev-edit,h-default,i-fp8,i-default'),"
SEED_H=$(param seed_h 32) SEED_I=$(param seed_i 7)
SRC=$PACKAGES/$TREE
for p in hidream-o1 ideogram-4; do [ -f "$SRC/$p/pyproject.toml" ] || fail "no $p in packages_refs checkout $TREE"; done
[[ $SEED_H =~ ^[0-9]+$ && $SEED_I =~ ^[0-9]+$ ]] || fail "seeds"
want() { [[ $ONLY == *",$1,"* ]]; }
# Upstream, pinned: the code and the checkpoints our packages' READMEs name. Ideogram's fp8 repository is gated, so its files
# come from cocktailpeanut's copy, which the Hub lists file by file with the same sha256 as ideogram-ai/ideogram-4-fp8@ee79a72
# (checked below, before anything loads).
HIDREAM_CODE=2c2d29ff729e48f33e41f49edfdbd81d5ac103b4 IDEOGRAM_CODE=990fe1c
HIDREAM_FULL=HiDream-ai/HiDream-O1-Image@0b0901d99f200389e138c61946af1185f5f49a13
HIDREAM_DEV=HiDream-ai/HiDream-O1-Image-Dev@c0bada0e15c54a9f96a6d1ecc35575b32bc21544
IDEOGRAM_OFFICIAL=ideogram-ai/ideogram-4-fp8@ee79a7237b519f1402ceacf952f30c8a31ec5073
IDEOGRAM_COPY=cocktailpeanut/ideogram-4-fp8@e0a0fb243cbf458fde53e358e80e6640cc593fd0
QWEN=Qwen/Qwen3-VL-8B-Instruct@0c351dd01ed87e9c1b53cbc748cba10e6187ff3b
# The cozy side's checkpoints for the parity cells: each slot pinned to its bf16 (or, for Ideogram's DiTs, fp8) checkpoint.
QWEN_BF16=qwen/qwen3-vl-8b-instruct@sha256:0b28073666896f0155006af7ea0c609d1dd541f40affcad61d9a1daa60dbd01c
for tool in jq uv git; do command -v "$tool" > /dev/null || fail "no $tool"; done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<an rc-dev run of rc/<x>>"

# Upstream's environment, built here where the network is good and shipped to the pod at the same path (as h3-vae does).
W=/var/tmp/upstream-parity
rm -rf "$W" && mkdir -p "$W" || fail "$W"
( set -e; cd "$W"; export UV_PYTHON_INSTALL_DIR=$W/python UV_PYTHON_PREFERENCE=only-managed UV_HTTP_TIMEOUT=300 UV_NO_CACHE=1
  P=(--python "$W/venv/bin/python" --python-platform x86_64-manylinux_2_28)
  git init -q hidream && git -C hidream fetch -q --depth 1 https://github.com/HiDream-ai/HiDream-O1-Image.git "$HIDREAM_CODE" && git -C hidream checkout -q FETCH_HEAD
  git clone -q https://github.com/ideogram-oss/ideogram4.git ideogram4 && git -C ideogram4 checkout -q "$IDEOGRAM_CODE"
  # HiDream pins Transformers 4.57.1 (requirements.txt). Ideogram's code calls Transformers 5's API (`create_causal_mask(inputs_embeds=)`)
  # and its tokenizer files are Transformers 5's, so it gets its own environment, on the ideogram-4 package's Transformers.
  tf5=$(awk '/^name = "transformers"$/{getline; print $3}' "$SRC/ideogram-4/uv.lock" | tr -d '"')
  for v in venv venv5; do
    uv venv -q --python 3.12 $v
    uv pip install -q --python "$W/$v/bin/python" --python-platform x86_64-manylinux_2_28 torch==2.14.1 torchvision==0.29.1 \
      --index-url https://download.pytorch.org/whl/cu130
  done
  # Diffusers for HiDream's schedulers: Tencent's 0.35.2, the newest on Transformers 4.57's huggingface_hub < 1.
  uv pip install -q "${P[@]}" transformers==4.57.1 diffusers==0.35.2 accelerate einops safetensors 'huggingface_hub[hf_xet]<1' \
    pillow numpy scipy tqdm
  uv pip install -q --python "$W/venv5/bin/python" --python-platform x86_64-manylinux_2_28 "transformers==$tf5" accelerate einops \
    safetensors 'huggingface_hub[hf_xet]' pillow numpy sentencepiece requests bitsandbytes
  cp "$FX"/hidream_ref.py "$FX"/ideogram_ref.py . && cp hidream/assets/edit/test.jpg edit.jpg
) > "$OUT/env.log" 2>&1
# Its own status, not `|| fail`: a subshell tested by `||` ignores its `set -e`.
[ $? = 0 ] || fail "upstream's environment: $(tail -5 "$OUT/env.log")"
say "upstream: HiDream $(git -C "$W/hidream" rev-parse --short HEAD) on $("$W/venv/bin/python" -I -c 'import torch, transformers; print("torch", torch.__version__, "transformers", transformers.__version__)'), \
Ideogram $(git -C "$W/ideogram4" rev-parse --short HEAD) on transformers $("$W/venv5/bin/python" -I -c 'import transformers; print(transformers.__version__)'): $(du -sh "$W" | cut -f1) to ship"
cp "$W/edit.jpg" "$OUT/edit.jpg"
# Both references import here, and Ideogram's tokenizer loads on its Transformers, before anything is rented.
( cd "$W" && venv/bin/python -I -c "import sys; sys.path.insert(0, 'hidream'); import inference, models.pipeline; print('hidream imports')" &&
  venv5/bin/python -I -c "
import sys; sys.path.insert(0, 'ideogram4/src')
import ideogram4.pipeline_ideogram4, transformers
from transformers import AutoTokenizer
t = AutoTokenizer.from_pretrained('${IDEOGRAM_COPY%@*}', subfolder='tokenizer', revision='${IDEOGRAM_COPY#*@}')
print('ideogram imports; tokenizer', type(t).__name__, len(t))" ) > "$OUT/imports.log" 2>&1 || fail "upstream's code does not import: $(tail -5 "$OUT/imports.log")"
say "$(grep -E 'imports' "$OUT/imports.log" | paste -sd' ')"
PROMPT_H=$(cat "$FX/hidream-t2i-prompt.txt") EDIT_H='remove the earphones from photo' CAPTION=$(cat "$FX/ideogram-caption.json")
jq -e . <<<"$CAPTION" > /dev/null || fail "the caption is no JSON"

# The scorer's Python (CPU torch), as upscale.sh's.
uv venv -q --python 3.12 "$OUT/../iqa" > "$OUT/iqa.log" 2>&1 &&
  uv pip install -q --python "$OUT/../iqa/bin/python" --torch-backend cpu pyiqa==0.1.16 'setuptools<81' >> "$OUT/iqa.log" 2>&1 ||
  fail "the scorer's Python: $(tail -c 400 "$OUT/iqa.log")"
IQA=$OUT/../iqa/bin/python

# Ideogram's copy is the official files: every LFS file's sha256 as the Hub lists it, both repositories.
lfs() { curl -fsS "https://huggingface.co/api/models/${1%@*}/revision/${1#*@}?blobs=true" |
  jq -r '.siblings[] | select(.lfs) | select(.rfilename | startswith("assets") | not) | "\(.rfilename) \(.lfs.sha256)"' | sort; }
lfs "$IDEOGRAM_OFFICIAL" > "$OUT/ideogram-official.lfs" && lfs "$IDEOGRAM_COPY" > "$OUT/ideogram-copy.lfs" &&
  [ -s "$OUT/ideogram-official.lfs" ] && cmp -s "$OUT/ideogram-official.lfs" "$OUT/ideogram-copy.lfs" ||
  fail "cocktailpeanut's Ideogram fp8 is not the official files: $(diff "$OUT/ideogram-official.lfs" "$OUT/ideogram-copy.lfs" | head -5)"
say "Ideogram's fp8: the copy's $(wc -l < "$OUT/ideogram-copy.lfs") weight files are the official ones, sha256 for sha256"
for p in hidream-o1 ideogram-4; do kernels "$SRC/$p"; done

[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s 4200)"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
pod "nvidia-smi --query-gpu=name,driver_version,memory.total --format=csv,noheader; df -h /var/tmp | tail -1; free -g | head -2" < /dev/null | tee -a "$R"

# Upstream's environment onto the pod, then its weights from Hugging Face in the background while our cells run.
t0=$SECONDS
tar -C "$W" -I "gzip -1" -cf - . | T=3600 pod "mkdir -p $W && tar -xzf - --no-same-owner -C $W" 2> "$OUT/ship.err" ||
  fail "shipping upstream's environment: $(tail -c 300 "$OUT/ship.err")"
say "upstream's environment on the pod in $((SECONDS - t0)) s"
PULL=$(cat <<EOF
import os, sys
from huggingface_hub import snapshot_download
hf = "$W/hf"
for spec, where, skip in (("$HIDREAM_FULL", "full", None), ("$HIDREAM_DEV", "dev", None), ("$QWEN", "qwen", ["*.md"]),
                          ("$IDEOGRAM_COPY", None, ["assets/*", "text_encoder/*"])):
    repo, rev = spec.split("@")
    if where:
        snapshot_download(repo, revision=rev, local_dir=os.path.join("$W", where), ignore_patterns=skip)
    else:  # Ideogram's code reads its repository through the hub cache, at main: main must be the pinned commit.
        got = snapshot_download(repo, revision="main", cache_dir=hf, ignore_patterns=skip)
        assert got.endswith(rev), (got, rev)
    print("pulled", spec, flush=True)
print("PULLED", flush=True)
EOF
)
printf '%s' "$PULL" | pod "cat > $W/pull.py && cd $W && { nohup venv/bin/python -I pull.py > pull.log 2>&1 < /dev/null & echo \$! > pull.pid; }" ||
  fail "starting the downloads"

# Ours: each cell one run followed to its end, its record and outputs kept.
cell() {
  local name=$1 N st; shift
  (cd "$SRC" && step "$name" run "$@" --on "$NAME") || fail "$name: $(tail -c 400 "$OUT/$name.txt")"
  N=$(runno "$name"); [ -n "$N" ] || fail "$name: no run: $(tail -c 400 "$OUT/$name.txt")"
  (cd "$RUN_PROJECT" && cozy run watch "$N" --once --json --full) > "$OUT/$name.json" 2> /dev/null
  (cd "$RUN_PROJECT" && cozy run watch "$N" --once -o "$OUT/$name.out") > /dev/null 2>&1
  st=$(jq -r '.status // "none"' "$OUT/$name.json")
  say "$name run $N $st: $(jq -r '[.models[]? | "\(.slot)=\(.model)@\(.release // "") \(.lane // "")"] | join(", ")' "$OUT/$name.json" 2> /dev/null) \
exec $(jq -r '(.execution_ms // 0) / 1000' "$OUT/$name.json")s; kernels $(jq -r '[.. | objects | select(has("kernel") and has("source")) | .kernel] | unique | join(" ")' "$OUT/$name.json" 2> /dev/null)"
  [ "$st" = completed ] || fail "$name $st: $(jq -r '.reason.message // .reason // empty' "$OUT/$name.json" | head -c 400)"
  cp "$(ls "$OUT/$name.out"/*.png | head -1)" "$OUT/ours-$name.png" || fail "$name: no picture in its outputs"
}
REF=(references:="[{\"name\": \"photo\", \"image\": \"$W/edit.jpg\"}]")
SDPA=(--attention-kernel sdpa)
want h-t2i && cell h-t2i ./hidream-o1/generate prompt="$PROMPT_H" seed:=$SEED_H guidance_interval:=false --lane bf16 "${SDPA[@]}"
want h-edit && cell h-edit ./hidream-o1/generate prompt='remove the earphones from <photo>' "${REF[@]}" seed:=$SEED_H \
  guidance_interval:=false --lane bf16 "${SDPA[@]}"
want h-dev-edit && cell h-dev-edit ./hidream-o1/generate_turbo prompt='remove the earphones from <photo>' "${REF[@]}" \
  seed:=$SEED_H --lane bf16 "${SDPA[@]}"
# What a user gets by default: the int8 lane, the default attention and limited-interval guidance.
want h-default && cell h-default ./hidream-o1/generate prompt="$PROMPT_H" seed:=$SEED_H
want i-fp8 && cell i-fp8 ./ideogram-4/generate prompt="$CAPTION" prompt_upsampling:=false seed:=$SEED_I --lane fp8 \
  --model "text_encoder=$QWEN_BF16" "${SDPA[@]}"
want i-default && cell i-default ./ideogram-4/generate prompt="$CAPTION" prompt_upsampling:=false seed:=$SEED_I

# Upstream alone on the GPU: a restart ends Cozy's executors. No Cozy run moves from here on, so the rental's idle clock is
# renewed by hand.
step free machine update "$NAME" --gpu-memory-limit off -y || say "freeing the GPU: $(tail -2 "$OUT/free.txt")"
( while sleep 240; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done ) 9>&- & BG=$!
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; sleep 15; done
for i in $(seq 1 120); do
  pod "grep -q PULLED $W/pull.log || ! kill -0 \$(cat $W/pull.pid)" < /dev/null > /dev/null 2>&1 && break; sleep 30
done
pod "tail -5 $W/pull.log; du -sh $W/full $W/dev $W/qwen $W/hf 2>/dev/null" < /dev/null | tee -a "$R"
pod "grep -q PULLED $W/pull.log" < /dev/null || fail "upstream's weights did not download: $(pod "tail -c 600 $W/pull.log" < /dev/null)"
# The copy's blobs are named by their sha256 in the hub cache: each the official one's.
pod "cd $W/hf && ls models--cocktailpeanut--ideogram-4-fp8/blobs" < /dev/null > "$OUT/ideogram-blobs.txt"
while read -r f sha; do grep -qx "$sha" "$OUT/ideogram-blobs.txt" || [[ $f == text_encoder/* ]] || fail "Ideogram's $f is not the official blob $sha"; done < "$OUT/ideogram-official.lfs"

h_cells() {
  jq -cn --arg p "$PROMPT_H" --arg e "$EDIT_H" --argjson s "$SEED_H" --arg ref "$W/edit.jpg" --arg which "$1" '
    if $which == "full" then [{name: "h-t2i", prompt: $p, seed: $s}, {name: "h-edit", prompt: $e, ref: $ref, keep_original_aspect: true, seed: $s}]
    else [{name: "h-dev-edit", prompt: $e, ref: $ref, keep_original_aspect: true, seed: $s}] end'
}
upstream() {  # upstream <label> <command run in $W on the pod>: a failure is said and the session goes on without its pictures
  if ! T=3600 pod "cd $W && $2" < /dev/null > "$OUT/up-$1.log" 2>&1; then
    say "upstream $1 FAILED: $(grep -v '^\s*$' "$OUT/up-$1.log" | tail -8 | cut -c1-300)"; RC=1
  fi
  grep '^{' "$OUT/up-$1.log" | while read -r l; do say "upstream $1: $l"; done
}
if want h-t2i || want h-edit || want h-default; then
  h_cells full | pod "cat > $W/h-full.json" || fail "cells"
  upstream h-full "venv/bin/python -I hidream_ref.py hidream full full h-full.json up"
fi
if want h-dev-edit; then
  h_cells dev | pod "cat > $W/h-dev.json" || fail "cells"
  upstream h-dev "venv/bin/python -I hidream_ref.py hidream dev dev h-dev.json up"
fi
if want i-fp8 || want i-default; then
  jq -cn --arg c "$CAPTION" --argjson s "$SEED_I" '[{name: "i-t2i", caption: $c, seed: $s}]' | pod "cat > $W/i.json" || fail "cells"
  upstream ideogram "HF_HOME=$W/hf HF_HUB_CACHE=$W/hf HF_HUB_OFFLINE=1 venv5/bin/python -I ideogram_ref.py ideogram4 ${IDEOGRAM_COPY%@*} qwen i.json up"
fi
pod "cd $W/up && tar -cf - *.png *.json *.pt 2>/dev/null" < /dev/null | (mkdir -p "$OUT/upstream" && tar -xf - -C "$OUT/upstream") ||
  say "fetching upstream's pictures: some are missing ($(ls "$OUT/upstream" 2> /dev/null | tr '\n' ' '))"
kill "$BG" 2> /dev/null; BG=''
# The scoring needs only the runner: the rental ends now (lib.sh's trap checks it is gone).
cozy machine end "$RID" -y >> "$OUT/end.txt" 2>&1; say "END requested $RID: every picture is in"

# Each of ours against upstream's.
U=$OUT/upstream
{ want h-t2i && printf 'h-t2i\t%s\t%s\n' "$U/h-t2i.png" "$OUT/ours-h-t2i.png"
  want h-edit && printf 'h-edit\t%s\t%s\n' "$U/h-edit.png" "$OUT/ours-h-edit.png"
  want h-dev-edit && printf 'h-dev-edit\t%s\t%s\n' "$U/h-dev-edit.png" "$OUT/ours-h-dev-edit.png"
  want h-default && printf 'h-default\t%s\t%s\n' "$U/h-t2i.png" "$OUT/ours-h-default.png"
  if want i-fp8; then
    printf 'i-fp8\t%s\t%s\n' "$U/i-t2i.png" "$OUT/ours-i-fp8.png"
    printf 'i-fp8-vs-bn-decode\t%s\t%s\n' "$U/i-t2i-bn.png" "$OUT/ours-i-fp8.png"
    printf 'i-upstream-bn-decode\t%s\t%s\n' "$U/i-t2i.png" "$U/i-t2i-bn.png"
  fi
  want i-default && printf 'i-default\t%s\t%s\n' "$U/i-t2i.png" "$OUT/ours-i-default.png"
} > "$OUT/pairs.tsv"
"$IQA" -I "$FX/compare.py" "$OUT/pairs.tsv" "$OUT/scores" > "$OUT/compare.log" 2>&1 || fail "scoring: $(tail -5 "$OUT/compare.log")"
say "scores (reference first; PSNR dB, mean |d| of 255, DISTS):"
column -t -s $'\t' "$OUT/scores/metrics.tsv" | tee -a "$R"
{ echo "### upstream-parity"; echo '```'; column -t -s $'\t' "$OUT/scores/metrics.tsv"; echo '```'; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
[ "$RC" = 0 ] || fail "an upstream run failed (above); what both sides made is scored"
say "PASS: every cell ran on both sides and is scored (scores/metrics.tsv, crop sheets in scores/)"
