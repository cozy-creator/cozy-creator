# hidream-forward (se-162): HiDream-O1-Image, the package's model (the RC's Runtime in plain Python, Transformers as its lock pins)
# against HiDream's own code (Transformers 4.57.1) on one small random-weight Qwen3-VL with the checkpoint's own processor files
# (fixtures/upstream-parity/hidream_forward.py): the full model's t2i, edit and two-reference composition, the Dev model's edit.
# No weights, no rental: $0, on the runner only.
# params:
#   tree   the packages_refs checkout (pk)
. "$(dirname "$0")/lib.sh"
FX=$(cd "$(dirname "$0")" && pwd)/fixtures/upstream-parity
TREE=$(param tree pk) PKG=$PACKAGES/$(param tree pk)/hidream-o1
[ -f "$PKG/pyproject.toml" ] || fail "no hidream-o1 in packages_refs checkout $TREE"
W=${RUNNER_TEMP:-$OUT/..}/hidream-forward REV=0b0901d99f200389e138c61946af1185f5f49a13
rm -rf "$W" && mkdir -p "$W/files" || fail "$W"
( set -e; cd "$W"
  git init -q hidream && git -C hidream fetch -q --depth 1 https://github.com/HiDream-ai/HiDream-O1-Image.git 2c2d29ff729e48f33e41f49edfdbd81d5ac103b4 && git -C hidream checkout -q FETCH_HEAD
  for f in config.json chat_template.json merges.txt preprocessor_config.json tokenizer.json tokenizer_config.json \
           video_preprocessor_config.json vocab.json generation_config.json; do
    curl -fsSL --retry 5 -o "files/$f" "https://huggingface.co/HiDream-ai/HiDream-O1-Image/resolve/$REV/$f"
  done
  uv venv -q --python 3.12 up && uv pip install -q --python up/bin/python --torch-backend cpu torch torchvision \
    transformers==4.57.1 diffusers==0.35.2 accelerate einops pillow numpy scipy tqdm
  lock() { awk -v n="$1" '$0 == "name = \"" n "\"" {getline; print $3}' "$PKG/uv.lock" | tr -d '"'; }
  rt=$(awk '/^name = "cozy-runtime"$/{getline; getline; print}' "$PKG/uv.lock" | sed 's/.*url = "\([^"]*\)".*/\1/')
  uv venv -q --python 3.12 pk && uv pip install -q --python pk/bin/python --torch-backend cpu torch torchvision \
    "cozy-runtime[media] @ $rt" "transformers==$(lock transformers)" "tokenizers==$(lock tokenizers)" "diffusers==$(lock diffusers)"
  echo "package env: $(pk/bin/python -I -c 'import transformers, diffusers, torch; print("transformers", transformers.__version__, "diffusers", diffusers.__version__, "torch", torch.__version__)')"
) > "$OUT/env.log" 2>&1
# Its own status, not `|| fail`: a subshell tested by `||` ignores its `set -e`.
[ $? = 0 ] || fail "environments: $(tail -5 "$OUT/env.log")"
say "$(grep 'package env' "$OUT/env.log")"
nice -n 19 "$W/up/bin/python" -I "$FX/hidream_forward.py" ref "$W/hidream" "$W/files" "$OUT" > "$OUT/ref.log" 2>&1 ||
  fail "HiDream's code: $(tail -6 "$OUT/ref.log")"
nice -n 19 "$W/pk/bin/python" -I "$FX/hidream_forward.py" port "$PKG" "$W/files" "$OUT" > "$OUT/port.log" 2>&1 ||
  fail "the package: $(tail -6 "$OUT/port.log")"
"$W/pk/bin/python" -I "$FX/hidream_forward.py" compare "$OUT" > "$OUT/compare.txt" 2>&1; ok=$?
while read -r l; do say "$l"; done < "$OUT/compare.txt"
rm -f "$OUT"/hidream-weights-*.pt
[ "$ok" = 0 ] || fail "the package's HiDream-O1 differs from HiDream's code (above)"
say "PASS: the package's HiDream-O1 is HiDream's code within rounding"
