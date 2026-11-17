# Suite `upscale` (se-147, se-149): each picture Upscale tool on small JPEGs of clean pictures, scored against the clean ones.
# se-147, Paul 11-17: "upscaling doesn't work. I attached the qwen image 2.1 upscaler to an image and ran it, and it just made the
# image more of the same. The image was small and had JPEG artifacts; those JPG artifacts just became more embellished, rather than
# fixed (the image was not upscaled at all properly)."
# Run when a release changes an upscaler package (UPSCALERS' packages); promotions of such a release need its PASS.
# Each upscaler's tool runs at its defaults, seed 42, on its INPUTS, the bench's hardest for it (sessions/fixtures/upscale-bench:
# 225 px JPEGs, q50-75): cell upscale-<key>-<input>. Meanwhile this runner builds the scorer's Python (CPU pyiqa) and the
# clean pictures (make_set.py, the originals from the private benchmarks checkout UPSCALE_ORIGINALS, sha256-checked).
# metrics.py scores each output against its clean picture; bars.py holds the upscaler's mean DISTS over its inputs at least
# BY below the Lanczos blow-up's (the margin measured in se-147's run): row upscale-<key>-dists, PASS or FAIL. An upscaler
# that brings a small JPEG no closer to the picture it came from than resampling does fails.
# UPSCALERS: <key> <package>/<function> <inputs, comma-separated> <BY>
# - qwen: the three it did worst on in se-147's runs (konqi-friends q50 4:2:0, hokusai-wave q55 4:2:0, kiki q65). Its fix
#   measured 53-60% under Lanczos' DISTS on them (pod-session 44073117278), the RC's tool 1% above (44057846726): the bar 30%.
# - seedvr2 (se-149, Paul 11-17: "audit the other upscalers to ensure they're actually doing useful upscaling ... to get
#   maximal results"): the three whose restoration gained most from 2160p (kiki q65, hokusai-wave q55 4:2:0, revoy-shichimi
#   q74). At 2160p, the fix's default, it measured 55% under Lanczos' DISTS on them; at the RC's 1080p 44% (pod-session
#   44058075454): the bar 50%.
UPSCALERS=(
  "qwen qwen-image-2/upscale konqi-friends,hokusai-wave,kiki 0.30"
  "seedvr2 seedvr2/upscale_image kiki,hokusai-wave,revoy-shichimi 0.50"
)
UPSCALE=$HERE/sessions/fixtures/upscale-bench UPSCALE_PY=${RUNNER_TEMP:-/tmp}/upscale-iqa
mkdir -p upscale/runs
( uv venv -q --python 3.12 "$UPSCALE_PY" && uv pip install -q --python "$UPSCALE_PY/bin/python" --torch-backend cpu pyiqa==0.1.16 'setuptools<81' &&
  "$UPSCALE_PY/bin/python" -I "$UPSCALE/make_set.py" --from "${UPSCALE_ORIGINALS:-/nonexistent}" --fetch --clean-only ) > upscale/scorer.log 2>&1 &
UPSCALE_SCORER=$!
for u in "${UPSCALERS[@]}"; do
  read -r key tool inputs by <<< "$u"
  for input in ${inputs//,/ }; do
    c=upscale-$key-$input
    cell "$c" "$(pkg "${tool%/*}" "${tool#*/}")" --file "image=$UPSCALE/degraded/$input.jpg" seed:=42
    f=$(output "$c" png)
    [ -n "$f" ] && mkdir -p "upscale/runs/$key" && cp "$f" "upscale/runs/$key/$input.png" || log "$c: no picture"
  done
done
if ! wait "$UPSCALE_SCORER"; then
  for u in "${UPSCALERS[@]}"; do expect "upscale-${u%% *}-dists" FAIL "no scorer: $(tail -c 300 upscale/scorer.log)"; done
else
  scored=$(for u in "${UPSCALERS[@]}"; do read -r _ _ inputs _ <<< "$u"; echo "$inputs"; done | paste -sd,)
  "$UPSCALE_PY/bin/python" -I "$UPSCALE/metrics.py" upscale/runs --images "$scored" --out upscale/scores.tsv > upscale/scores.txt 2>&1 ||
    log "upscale: metrics.py: $(tail -c 300 upscale/scores.txt)"
  for u in "${UPSCALERS[@]}"; do
    read -r key tool inputs by <<< "$u"
    grep -q "^upscale-$key-" cells.tsv || { expect "upscale-$key-dists" SKIP "${tool%/*}: not run (--only)"; continue; }
    if [ ! -d "upscale/runs/$key" ]; then expect "upscale-$key-dists" FAIL "${tool%/*}: no picture"; continue; fi
    line=$("$UPSCALE_PY/bin/python" -I "$UPSCALE/bars.py" upscale/scores.tsv "[{\"candidate\": \"$key\", \"vs\": \"lanczos\", \"metric\": \"dists\", \"by\": $by}]" 2>&1)
    expect "upscale-$key-dists" "$([[ $line == PASS* ]] && echo PASS || echo FAIL)" "${tool%/*}: ${line#* }"
  done
fi
