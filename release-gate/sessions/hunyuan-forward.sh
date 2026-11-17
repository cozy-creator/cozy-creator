# hunyuan-forward (se-162): HunyuanImage 3.0's image stage, the package's vendored port (Transformers 5.23, the RC's Runtime
# in plain Python) against Tencent's own code (Transformers 4.57.1), on one small random-weight model with the same weights and
# noise (fixtures/upstream-parity/hunyuan_forward.py): Instruct (CFG) and Distil (guidance and meanflow tokens), t2i, an edit
# and a fusion. Tencent's CPU proof went with the package's new noise source on 11-06 (efb65da2); this is its successor for
# what changed since: K/V through `retained`, RoPE per call, the attention site, the Runtime's experts. No weights, no
# rental: $0, on the runner only. The grouped expert kernels and GPU attention kernels are not exercised (CPU).
# params:
#   tree   the packages_refs checkout (pk)
. "$(dirname "$0")/lib.sh"
FX=$(cd "$(dirname "$0")" && pwd)/fixtures/upstream-parity
TREE=$(param tree pk) PKG=$PACKAGES/$(param tree pk)/hunyuan-image-3
[ -f "$PKG/pyproject.toml" ] || fail "no hunyuan-image-3 in packages_refs checkout $TREE"
W=${RUNNER_TEMP:-$OUT/..}/hunyuan-forward
rm -rf "$W" && mkdir -p "$W/files" || fail "$W"
( set -e; cd "$W"
  git clone -q https://github.com/Tencent-Hunyuan/HunyuanImage-3.0.git tencent && git -C tencent checkout -q 6e9113a6
  uv venv -q --python 3.12 up && uv pip install -q --python up/bin/python --torch-backend cpu torch torchvision \
    transformers==4.57.1 diffusers==0.35.2 einops pillow numpy accelerate
  lock() { awk -v n="$1" '$0 == "name = \"" n "\"" {getline; print $3}' "$PKG/uv.lock" | tr -d '"'; }
  rt=$(awk '/^name = "cozy-runtime"$/{getline; getline; print}' "$PKG/uv.lock" | sed 's/.*url = "\([^"]*\)".*/\1/')
  uv venv -q --python 3.12 pk && uv pip install -q --python pk/bin/python --torch-backend cpu torch torchvision \
    "cozy-runtime[media] @ $rt" "transformers==$(lock transformers)" "tokenizers==$(lock tokenizers)" einops
  echo "package env: $(pk/bin/python -I -c 'import cozy_runtime, transformers, torch; print("cozy-runtime", cozy_runtime.__version__ if hasattr(cozy_runtime, "__version__") else "?", "transformers", transformers.__version__, "torch", torch.__version__)')"
) > "$OUT/env.log" 2>&1
# Its own status, not `|| fail`: a subshell tested by `||` ignores its `set -e`.
[ $? = 0 ] || fail "environments: $(tail -5 "$OUT/env.log")"
say "$(grep 'package env' "$OUT/env.log")"
f=$W/files
for n in config.json generation_config.json tokenizer.json tokenizer_config.json; do
  curl -fsSL --retry 5 -o "$f/$n" "https://huggingface.co/tencent/HunyuanImage-3.0-Instruct/resolve/2ec2c78bee7d4b94157341fba86c4c2c7b1858b2/$n" || fail "$n"
done
bad=0
for kind in instruct distil; do
  nice -n 19 "$W/up/bin/python" -I "$FX/hunyuan_forward.py" ref "$W/tencent/hunyuan_image_3" "$f" "$kind" "$OUT" > "$OUT/ref-$kind.log" 2>&1 ||
    fail "Tencent's $kind: $(tail -6 "$OUT/ref-$kind.log")"
  nice -n 19 "$W/pk/bin/python" -I "$FX/hunyuan_forward.py" port "$PKG" "$f" "$kind" "$OUT" > "$OUT/port-$kind.log" 2>&1 ||
    fail "the package's $kind: $(tail -6 "$OUT/port-$kind.log")"
  "$W/pk/bin/python" -I "$FX/hunyuan_forward.py" compare "$OUT" "$kind" > "$OUT/compare-$kind.txt" 2>&1 || bad=$((bad + 1))
  while read -r l; do say "$l"; done < "$OUT/compare-$kind.txt"
done
rm -f "$OUT"/weights-*.pt
[ "$bad" = 0 ] || fail "the package's image stage differs from Tencent's (above)"
say "PASS: the package's image stage is Tencent's within rounding"
