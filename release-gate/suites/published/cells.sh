# Suite `published` (cl-1160; tracker design/release-tests-v2.md, E1 row 0): version skew, cozy × packages, every RC. The candidate
# cozy runs what users have until the RC is promoted: each first-party package's published release, its default call
# (`<cell>-published`), on the Runtime and TensorFS the pod booted with (the Hub's), whatever candidates the gate put on. H3's takes a
# character reference, so its segments are managed calls with files: the 0.26.0 RC's TensorD refused Runtime 0.46's (44053323560).
PUBLISHED_KEEP=$PKGS
read -r PUBLISHED_RT PUBLISHED_TFS <<< "$(awk '$1 == "runtime" {r = $2} $1 == "tensorfs" {t = $2} END {print r, t}' machine-fresh.txt)"
PUBLISHED_MOVED=
if [ -n "$PUBLISHED_RT" ] && [ -n "$PUBLISHED_TFS" ] && [ "$PUBLISHED_RT $PUBLISHED_TFS" != "$rt $tfs" ]; then
  # The gate's cozy stays: cozy-gate names it on every update.
  if "$CLI" --json machine update "$M" --runtime "$PUBLISHED_RT" --tensorfs "$PUBLISHED_TFS" -y > published-pair.json 2>&1; then
    PUBLISHED_MOVED=1
  else
    expect published-pair FAIL "$M didn't move to the Runtime $PUBLISHED_RT and TensorFS $PUBLISHED_TFS it booted with: $(tail -c 300 published-pair.json)"
  fi
fi
log "published: on Runtime ${PUBLISHED_RT:-?}, TensorFS ${PUBLISHED_TFS:-?}"
# Each package's default call: the first cell of its product_* (products.sh).
TWINS=$(for p in "${PRODUCTS[@]}"; do declare -f "product_${p//[-.]/_}" | awk '$1 == "cell" {print $2; exit}'; done | paste -sd, -)
TWIN=-published PKGS=
for p in "${PRODUCTS[@]}"; do product "$p"; done
TWIN= TWINS= PKGS=$PUBLISHED_KEEP
[ -z "$PUBLISHED_MOVED" ] || "$CLI" --json machine update "$M" ${RWF:+--runtime "$RWF"} ${TWF:+--tensorfs "$TWF"} -y > published-back.json 2>&1 \
  || expect published-back FAIL "$M didn't take its candidates back: $(tail -c 300 published-back.json)"
