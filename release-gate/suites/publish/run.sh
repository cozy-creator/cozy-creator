#!/usr/bin/env bash
# Suite `publish` (xs-194), the Hub-publish path a gate's source runs skip: e2e/skew/run.sh --admin --command OUT COZY RUNTIME
# TENSORFS PACKAGES [ONLY]. On CI's throwaway Hub, signed in as its admin, the packages checkout's candidates (each whose
# version is a prerelease, or ONLY's) are published as the promotion publishes them to tensorhub.com, by the checkout's
# `task publish -- --hub`: finals, with the in-repo packages they take, relocked against this Hub's index, their Triton
# sets prebuilt, described. Then each is installed on this computer's machine from the Hub's index (its lock rebound to
# it, its `_triton` in the environment), and BiRefNet, published the same way with its weights mirrored from the Hugging
# Face revision production's are, runs once on this CPU. Nothing reaches tensorhub.com.
set -uo pipefail
OUT=$1 COZY_BIN=$2 RT=$3 TF=$4 PKGS=$(cd "$5" && pwd) ONLY=${6:-}
HERE=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$OUT" && cd "$OUT" || exit 2
: > expect.tsv
log() { echo "$(date -u +%H:%M:%SZ) $*" | tee -a gate.log; }
expect() { printf '%s\t%s\t%s\n' "$1" "$2" "$(tr '\n\t\r' '   ' <<< "$3" | cut -c1-400)" >> expect.tsv; log "$1: $2 $3"; }
verdict() {
  local fails passes
  fails=$(grep -c $'\tFAIL\t' expect.tsv) passes=$(grep -c $'\tPASS\t' expect.tsv)
  grep $'\tFAIL\t' expect.tsv | cut -f1,3 | sed 's/^/FAIL /' | tee verdict.txt
  if [ "$fails" = 0 ] && [ "$passes" -gt 0 ]; then echo "VERDICT PASS ($passes cells)" | tee -a verdict.txt; exit 0; fi
  echo "VERDICT FAIL ($fails of $((fails + passes)) cells)" | tee -a verdict.txt; exit 1
}
export PATH=$(dirname "$COZY_BIN"):$PATH
. "$HERE/e2e/test-hub.sh"
require_test_hub "${COZY_HUB_URL:-}" || { expect publish-hub FAIL "not the throwaway Hub: ${COZY_HUB_URL:-unset}"; verdict; }
J() { jq -e "$@" > /dev/null; }
ver() { grep -m1 '^version = ' "$PKGS/$1/pyproject.toml" | cut -d'"' -f2; }
final() { ver "$1" | sed -E 's/(a|b|rc)[0-9]+$//; s/\.dev[0-9]+$//'; }
deps() { sed -n 's/^\([A-Za-z0-9._-]*\) = { *index = "tensorhub".*/\1/p' "$PKGS/$1/pyproject.toml"; }
# uv (task publish's locks, Calcifer's relock to the Hub's advertised index) reads the Hub's own CA.
sudo cp "${COZY_TEST_HUB_CA:?}" /usr/local/share/ca-certificates/cozy-test-hub.crt && sudo update-ca-certificates > ca.log 2>&1
export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
systemctl --user set-environment SSL_CERT_FILE="$SSL_CERT_FILE"
systemctl --user restart cozy-calcifer > calcifer-restart.log 2>&1 || true
for _ in $(seq 30); do cozy --json whoami > /dev/null 2>&1 && break; sleep 2; done
[ -z "$RT$TF" ] || cozy --json machine update local ${RT:+--runtime "$RT"} ${TF:+--tensorfs "$TF"} -y > machine-update.json 2>&1 \
  || { expect publish-candidates FAIL "the machine refused the candidates: $(tail -c 300 machine-update.json)"; verdict; }

. "$HERE/e2e/run.sh"
run_project "$OUT/project" || { expect publish-candidates FAIL "no project"; verdict; }
if [ -n "$ONLY" ]; then cands=${ONLY//,/ }
else
  cands=$(for f in "$PKGS"/*/pyproject.toml; do
    grep -qs cozy.application "$f" && grep -m1 '^version = ' "$f" | grep -qE '(a|b|rc)[0-9]+"|\.dev[0-9]+"' && basename "$(dirname "$f")"
  done | paste -sd' ')
fi
[ -n "$cands" ] || { expect publish-candidates FAIL "no package in the checkout is a candidate (a prerelease version)"; verdict; }
log "candidates: $cands, and birefnet to run"

# The promotion's own publish, aimed at this Hub, one candidate at a time so each is its own cell (in-repo packages it
# takes come with it): its commits stay in this checkout, which it leaves as it was.
git -C "$PKGS" config user.name "Paul Fidika"; git -C "$PKGS" config user.email paul@fidika.com
here() { cozy --json package ls "$1" --limit 200 2>/dev/null | jq -r --arg p "_/$1" --arg q "$1" '[.packages[] | select(.package == $p or .package == $q)][0].latest_release // empty'; }
published=''
for p in $cands birefnet; do
  if [ "$(here "$p")" = "$(final "$p")" ]; then expect "publish-$p" PASS "$p $(final "$p") published already, with a package that takes it"; published="$published $p"; continue; fi
  (cd "$PKGS" && task publish -- --hub "$COZY_HUB_URL" "$p") > "publish-$p.log" 2>&1; code=$?
  shipped=$(sed -n 's/^published on .* only, nothing pushed: //p' "publish-$p.log" | tail -1)
  if [ "$code" = 0 ] && [ -n "$shipped" ]; then
    expect "publish-$p" PASS "task publish -- --hub: $shipped$(grep -E ': [0-9]+ kernels, ' "publish-$p.log" | sed 's/^/; /' | paste -sd' ')"
    published="$published $p"
  else expect "publish-$p" FAIL "task publish -- --hub $p exit $code: $(grep -vE '^\s*$' "publish-$p.log" | tail -n 3 | paste -sd' ')"; fi
done

# Each candidate, installed from the Hub's index: its release is the final, its lock rebound here, its Triton sets in its
# environment beside the module that declares them.
generations() { find "$HOME/.cozy" "$HOME/.local/share" /var/lib/cozy -path '*/generations/*/generation.json' 2>/dev/null; }
for p in $cands; do
  [[ " $published " == *" $p "* ]] || continue
  want=$(final "$p")
  (cd "$RUN_PROJECT" && cozy --json package install "$p@$want" --on local -y) > "install-$p.json" 2>&1; code=$?
  if [ "$code" != 0 ]; then expect "publish-install-$p" FAIL "package install $p@$want exit $code: $(tail -c 300 "install-$p.json")"; continue; fi
  mod=$(sed -n '/cozy\.application/,/^\[/s/^default = "\([A-Za-z0-9_]*\)[.:].*/\1/p' "$PKGS/$p/pyproject.toml")
  triton=none
  if grep -rqs --include='*.py' 'TritonKernels(' "$PKGS/$p"; then
    triton=missing
    for g in $(generations); do
      m=$(find "$(dirname "$g")/env/" -path "*/site-packages/$mod/_triton/manifest.json" 2>/dev/null | head -1)
      [ -n "$m" ] && triton="$(jq '.kernels | length' "$m") kernels" && break
    done
  fi
  bound=''
  [ -z "$(deps "$p")" ] || bound=" (takes $(deps "$p" | paste -sd' ') from this Hub's index)"
  if [ "$triton" = missing ]; then expect "publish-install-$p" FAIL "$p@$want installed$bound, but its environment holds no $mod/_triton"
  else expect "publish-install-$p" PASS "$p@$want installed from the Hub's index$bound; _triton: $triton"; fi
done

# One function run from the Hub: BiRefNet, its weights mirrored here first.
[[ " $published " == *" birefnet "* ]] || verdict
if (cd "$RUN_PROJECT" && cozy --json model upload hf://ZhengPeng7/BiRefNet@e2bf8e4460fc8fa32bba5ea4d94b3233d367b0e4 zhengpeng7/birefnet --lane fp16 \
    --mirror https://huggingface.co/ZhengPeng7/BiRefNet --on local -y) > upload-birefnet.json 2>&1 \
  && commit=$(python3 "$HERE/e2e/model-details.py" zhengpeng7/birefnet | jq -r '.checkpoints[0].checkpoint // empty') && [ -n "$commit" ] \
  && cozy --json model publish zhengpeng7/birefnet 0.1.0 "$commit" -y > publish-birefnet-model.json 2>&1; then
  log "BiRefNet's weights mirrored: zhengpeng7/birefnet@0.1.0 ($commit)"
else
  expect publish-run FAIL "BiRefNet's weights did not mirror: $(tail -c 300 upload-birefnet.json) $(tail -c 200 publish-birefnet-model.json 2>/dev/null)"; verdict
fi
picture=$HERE/release-gate/ref/seed.jpg
target=birefnet/remove_background@$(final birefnet)
if run_cell "$OUT" publish-run "$target" --file "image=$picture" --on local; then
  out=$(run_output "$OUT" publish-run 'png|webp')
  [ -s "$out" ] && expect publish-run PASS "run $(cat publish-run.number) of $target from the Hub: $(basename "$out")" \
    || expect publish-run FAIL "run $(cat publish-run.number) of $target completed with no picture"
else
  expect publish-run FAIL "run $(cat publish-run.number 2>/dev/null) of $target: $(run_get "$OUT" publish-run .reason.message | head -c 300)"
fi
verdict
