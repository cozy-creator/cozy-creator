#!/bin/bash
# interface-read (proof only, #4000; never merged): a packages checkout's package described as a gate describes it (release-gate/
# interfaces.sh --out, the file ledger.py reads), and its functions' request defaults said. $0: it rents nothing.
# params: runtime (0.48.0), tensorfs (0.27.0), package (relayvsr), function (upscale); packages_refs pk=<commit>.
set -uo pipefail
mkdir -p "$OUT"; R=$OUT/results.txt
say() { echo "$(date -u +%T) $*" | tee -a "$R"; }
p() { jq -r --arg k "$1" --arg d "$2" '.[$k] // $d' <<< "$PARAMS"; }
RT=$(p runtime 0.48.0) TF=$(p tensorfs 0.27.0) PKG=$(p package relayvsr) FN=$(p function upscale)
HERE=$(cd "$(dirname "$0")/.." && pwd)
ref=$(git -C "$HERE" rev-parse HEAD)
for f in interfaces.sh local-sources.sh; do
  gh api "repos/cozy-creator/cozy-creator/contents/release-gate/$f?ref=$ref" -H 'Accept: application/vnd.github.raw' > "$HERE/$f" && chmod +x "$HERE/$f" ||
    { say "FAIL: no $f at $ref"; exit 1; }
done
bash "$HERE/interfaces.sh" --runtime "$RT" --tensorfs "$TF" --packages "$PACKAGES/pk" --out "$OUT/interfaces" "$PACKAGES/pk/$PKG" 2>&1 | tee -a "$R"
f=$OUT/interfaces/$PKG.json
[ -s "$f" ] || { say "FAIL: no $f"; exit 1; }
say "$PKG $FN invocable.defaults: $(jq -c --arg fn "$FN" '.entrypoints[] | select(.name == $fn) | .invocable.defaults' "$f")"
say "$PKG $FN request fields: $(jq -c --arg fn "$FN" '[.entrypoints[] | select(.name == $fn) | .request.fields[] | {name, default}]' "$f")"
say "RESULT PASSED"
