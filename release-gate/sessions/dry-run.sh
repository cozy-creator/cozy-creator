# dry-run (var-036, proof only, never merged): H3 15 s Turbo's DiT forward on fake (shape-only) tensors, on this runner's
# own CPU: no rental, $0 (dry-run.py). Evidence is dry/dry-run.json (per block and per split site: bytes live, peaks, the
# package estimate beside them, seconds) and dry/timeline-block0-c8-steady.json. It judges nothing.
# params:
#   tree  the packages_refs checkout whose minimax-h3 runs (`-f packages_refs=pk=<packages ref>`; default pk)
#   torch a torch version for the CPU wheel (default: the newest)
# Dispatch with `-f runtime_ref=<cozy-runtime ref>`: the Runtime is that wheel (RUNTIME_WHEEL).
. "$(dirname "$0")/lib.sh"
TREE=$(param tree pk) TORCH=$(param torch '')
SRC=$PACKAGES/$TREE/minimax-h3
[ -f "$SRC/pyproject.toml" ] || fail "dispatch with -f packages_refs=$TREE=<packages ref>"
[ -n "${RUNTIME_WHEEL:-}" ] && [ -f "$RUNTIME_WHEEL" ] || fail "no RUNTIME_WHEEL: dispatch with -f runtime_ref=<cozy-runtime ref>"
V=$RUNNER_TEMP/dry-venv
say "minimax-h3 from $TREE $(git -C "$SRC" rev-parse --short=8 HEAD 2> /dev/null); Runtime $(basename "$RUNTIME_WHEEL"); $(nproc) CPUs"
nice -n 19 uv venv -q -p 3.12 "$V" > "$OUT/venv.txt" 2>&1 || fail "uv venv: $(tail -c 300 "$OUT/venv.txt")"
nice -n 19 uv pip install -q -p "$V" --index-url https://download.pytorch.org/whl/cpu "torch${TORCH:+==$TORCH}" > "$OUT/pip-torch.txt" 2>&1 ||
  fail "torch: $(tail -c 400 "$OUT/pip-torch.txt")"
nice -n 19 uv pip install -q -p "$V" diffusers==0.41.0 transformers==5.19.0 tokenizers safetensors msgspec numpy packaging pillow regex \
  pyyaml huggingface-hub > "$OUT/pip-deps.txt" 2>&1 || fail "deps: $(tail -c 400 "$OUT/pip-deps.txt")"
# The packages' lock takes the RC's TensorFS by URL; the Runtime only imports it here.
TFS=https://github.com/cozy-creator/cozy-creator/releases/download/rc-0.27.0/tensorfs-0.27.0.dev3399+g1d8eaf3b-cp311-abi3-manylinux_2_28_x86_64.whl
nice -n 19 uv pip install -q -p "$V" --no-deps "$RUNTIME_WHEEL" "$TFS" > "$OUT/pip-runtime.txt" 2>&1 ||
  nice -n 19 uv pip install -q -p "$V" --no-deps "$RUNTIME_WHEEL" tensorfs >> "$OUT/pip-runtime.txt" 2>&1 ||
  fail "runtime: $(tail -c 400 "$OUT/pip-runtime.txt")"
"$V/bin/python" -c 'from importlib.metadata import version as v; print(*(f"{n} {v(n)}" for n in ("torch", "diffusers", "tensorfs", "cozy-runtime")))' \
  2>&1 | tee -a "$R"
uv pip freeze -p "$V" > "$OUT/freeze.txt" 2>&1
started=$(date +%s)
nice -n 19 timeout 3000 "$V/bin/python" "$(dirname "$0")/dry-run.py" "$SRC" "$OUT/dry" > "$OUT/dry-run.log" 2>&1; code=$?
say "dry-run.py exit $code after $(($(date +%s) - started)) s; its last lines:"
tail -n 40 "$OUT/dry-run.log" | tee -a "$R"
[ "$code" = 0 ] && [ -f "$OUT/dry/dry-run.json" ] || fail "dry-run.py"
