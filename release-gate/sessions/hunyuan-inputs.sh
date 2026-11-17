# hunyuan-inputs (se-162): HunyuanImage 3.0's model inputs as the package's vendored code builds them on Transformers 5.23
# against Tencent's own code on Transformers 4.57.1 (fixtures/upstream-parity/hunyuan_inputs.py): token ids, image slices,
# masks, scatter indices, the reference images' VAE and ViT tensors and the size table, both checkpoints. No weights and
# no rental: the model is 168 GB, past what a parity check may rent. $0, on the runner only.
# params:
#   tree   the packages_refs checkout (pk)
. "$(dirname "$0")/lib.sh"
FX=$(cd "$(dirname "$0")" && pwd)/fixtures/upstream-parity
TREE=$(param tree pk) PKG=$PACKAGES/$(param tree pk)/hunyuan-image-3
[ -f "$PKG/pyproject.toml" ] || fail "no hunyuan-image-3 in packages_refs checkout $TREE"
TENCENT=6e9113a6
W=${RUNNER_TEMP:-$OUT/..}/hunyuan-inputs
rm -rf "$W" && mkdir -p "$W/files" || fail "$W"
# Tencent's environment: its requirements' Transformers and Diffusers, CPU torch.
( set -e; cd "$W"
  git clone -q https://github.com/Tencent-Hunyuan/HunyuanImage-3.0.git tencent && git -C tencent checkout -q "$TENCENT"
  uv venv -q --python 3.12 up && uv pip install -q --python up/bin/python --torch-backend cpu torch torchvision \
    transformers==4.57.1 diffusers==0.35.2 einops pillow numpy accelerate
  # The package's, as its lock pins Transformers and tokenizers (the vendored preprocessing imports no Runtime).
  tf=$(awk '/^name = "transformers"$/{getline; print $3}' "$PKG/uv.lock" | tr -d '"')
  tk=$(awk '/^name = "tokenizers"$/{getline; print $3}' "$PKG/uv.lock" | tr -d '"')
  uv venv -q --python 3.12 pk && uv pip install -q --python pk/bin/python --torch-backend cpu torch torchvision \
    "transformers==$tf" "tokenizers==$tk" einops pillow numpy
  echo "package env: transformers $tf, tokenizers $tk"
) > "$OUT/env.log" 2>&1
# Its own status, not `|| fail`: a subshell tested by `||` ignores its `set -e`.
[ $? = 0 ] || fail "environments: $(tail -5 "$OUT/env.log")"
say "$(grep 'package env' "$OUT/env.log"); Tencent $(git -C "$W/tencent" rev-parse --short HEAD)"
bad=0
for spec in tencent/HunyuanImage-3.0-Instruct@2ec2c78bee7d4b94157341fba86c4c2c7b1858b2 \
            tencent/HunyuanImage-3.0-Instruct-Distil@c8ffd07206f1b843697606968196e8f59f8ff38c; do
  repo=${spec%@*} rev=${spec#*@} n=${repo##*-}; d=$W/files/$n; mkdir -p "$d"
  for f in config.json generation_config.json tokenizer.json tokenizer_config.json; do
    curl -fsSL --retry 5 -o "$d/$f" "https://huggingface.co/$repo/resolve/$rev/$f" || fail "$repo $f"
  done
  "$W/up/bin/python" -I "$FX/hunyuan_inputs.py" dump "$W/tencent/hunyuan_image_3" "$d" upstream "$OUT/$n-upstream.pt" > "$OUT/$n-upstream.log" 2>&1 ||
    fail "$n upstream: $(tail -5 "$OUT/$n-upstream.log")"
  "$W/pk/bin/python" -I "$FX/hunyuan_inputs.py" dump "$PKG/hunyuan_image_3/tencent" "$d" package "$OUT/$n-package.pt" > "$OUT/$n-package.log" 2>&1 ||
    fail "$n package: $(tail -5 "$OUT/$n-package.log")"
  "$W/pk/bin/python" -I "$FX/hunyuan_inputs.py" compare "$OUT/$n-upstream.pt" "$OUT/$n-package.pt" > "$OUT/$n-compare.txt" 2>&1 || bad=$((bad + 1))
  say "$repo: $(tail -1 "$OUT/$n-compare.txt")"
  grep '^DIFF' "$OUT/$n-compare.txt" | head -20 | while read -r l; do say "  $l"; done
done
[ "$bad" = 0 ] || fail "the package's inputs differ from Tencent's in $bad checkpoint(s)"
say "PASS: the package builds Tencent's inputs, value for value"
