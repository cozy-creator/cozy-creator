# Suite `attention-corpus` (README.md, xs-171 M-B): real Q/K/V from our models, for the attention suite. Each model runs its
# bench request twice on this pod with the dev-only `cozy run --attention-capture`: the census alone (every attention call's
# site, shape, band and kernel), then the capture of the sites and calls corpus.py picks from it, same request and seed. The
# captures come back over SSH into corpus/ (this gate's artifact), and corpus.py checks each against its census row.
AC=$HERE/suites/attention-corpus AC_DIR=/tmp/attention-corpus
# The capture is cozy 0.25's: an earlier cozy has no such option, so the suite has nothing to run on it.
# (Its refusal is read whole: under gate.sh's pipefail, a pipe from cozy's failing exit never matches.)
if grep -q "unexpected argument '--attention-capture'" <<< "$("$CLI" run --attention-capture '{}' 2>&1)"; then
  expect attention-corpus SKIP "cozy $("$CLI" --version | awk '{print $2}') has no \`cozy run --attention-capture\` (0.25 and later): nothing captured"
  return 0
fi
# A dev-only option needs `dev = true` at the top of this home's config (a runner signed in by key has no config yet, so
# no `cat` of it may stop the write: release gate 43019157186).
ac_cfg=$HOME/.cozy/config.toml
grep -qxE 'dev *= *true' "$ac_cfg" 2>/dev/null ||
  { mkdir -p "${ac_cfg%/*}" && { echo 'dev = true'; [ ! -f "$ac_cfg" ] || cat "$ac_cfg"; } > "$ac_cfg.new" && mv "$ac_cfg.new" "$ac_cfg"; } ||
  log "could not write dev = true into $ac_cfg"
ac_seed=$((R + 720))
# ac_model <name> <package/function> [args...]: its census run, then its capture run, of the package users get, or its
# --packages checkout's (`pkg`: a published release may lock an older Runtime than the candidate's).
ac_model() {
  local name=$1 target seed=$((ac_seed++)) spec; target=$(pkg "${2%/*}" "${2##*/}"); shift 2
  cell "ac-$name-census" "$target" "$@" seed:=$seed --attention-capture "{\"dir\":\"$AC_DIR/census-$name\"}"
  [ "$(run_get "$OUT" "ac-$name-census" .status)" = completed ] || return 0
  "$CLI" machine ssh "$M" -- "cat $AC_DIR/census-$name/*/census.jsonl" < /dev/null > "ac-$name-census.jsonl" 2> "ac-$name.err"
  if ! spec=$(python3 -I "$AC/corpus.py" pick "$AC_DIR/$name" "ac-$name-census.jsonl" 2>> "ac-$name.err"); then
    expect "ac-$name" FAIL "no census from run $(cat "ac-$name-census.number"): $(tail -c 300 "ac-$name.err")"
    return 0
  fi
  log "ac-$name: $(tail -n 1 "ac-$name.err")"
  cell "ac-$name" "$target" "$@" seed:=$seed --attention-capture "$spec"
}
# The largest download first: a host too slow for the cap is found on it (gate.sh hold, too_slow) and replaced before
# the smaller models spend its time (release gate 43020656677: 2 MB/s, canceled).
ac_model h3-15s minimax-h3/generate beats:='[{"description":"[Shot 1]\nA slow bank of fog crosses the bay. The camera tracks along the cliff edge while gulls ride the wind. Spray climbs the rocks below the gallery.","duration":15}]' \
  soundscape="Wind and ocean surf, with waves splashing against the rocks." turbo:=true --gpus=1
ac_model qwen4mp qwen-image-2/generate prompt="A rooftop garden in a rainy megacity, neon reflections, fine detail" megapixels:=4 \
  prompt_upsampling:=false --gpus=1
ac_model moss moss-image/generate prompt="A hot air balloon festival over a desert canyon at dawn" megapixels:=4 aspect_ratio=1:1 \
  steps:=50 guidance:=3 --gpus=1
ac_model klein flux.3/generate prompt="A ceramic fox figurine on a mossy stone, macro photo, soft window light" \
  prompt_upsampling:=false prompt_drafting:=false --gpus=1
# The captures, over SSH. No run moves meanwhile, so the rental's idle clock is renewed by hand.
mkdir -p corpus
( while sleep 120; do "$CLI" machine keepalive "$M" > /dev/null 2>&1; done ) & ac_keep=$!
"$CLI" machine ssh "$M" -- "tar -C $AC_DIR --exclude='./census-*' -cf - ." < /dev/null 2> corpus.err | tar -C corpus -xf - 2>> corpus.err
kill "$ac_keep" 2> /dev/null
log "corpus: $(du -sh corpus | cut -f1) in $(find corpus -name '*.safetensors' | wc -l) captures"
python3 -I "$AC/corpus.py" summary corpus > corpus-summary.md 2>&1
if uv run -q --no-project --with numpy --with torch --index https://download.pytorch.org/whl/cpu --index-strategy unsafe-best-match \
  python -I "$AC/corpus.py" check corpus > corpus-check.txt 2>&1; then
  expect attention-corpus PASS "$(grep -v '^FAIL' corpus-check.txt | paste -sd';' | cut -c1-380)"
else
  expect attention-corpus FAIL "$(grep '^FAIL' corpus-check.txt | head -3 | paste -sd';') $(tail -c 200 corpus.err)"
fi
