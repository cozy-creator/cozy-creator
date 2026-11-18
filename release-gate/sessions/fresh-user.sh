# fresh-user (0.27.0 launch, coordinator 11-18): a brand-new user's first run on this computer, with NO dev home and NO packages checkout.
# The RC's cozy as installed (pod-session `cozy`), the installer's own `cozy doctor --fix`, signed in, `cozy new`, then PUBLISHED packages
# from the Hub on `local` (this runner's CPU): birefnet on a small picture, then qwen3's Write on a 0.8B model. Each step's words and time.
# A refused run is followed as a user would: `cozy machine update local`, then the same run again. The page's view of the same box
# (e2e/page.py, then a screenshot in Chrome). With `source` (a packages_refs checkout name), the same afterwards from that checkout in a
# dev home, the comparison arm. Proof-branch harness (release/walkthrough-027), never merged. $0: nothing is rented.
#   source   optional packages_refs checkout name for the comparison arm
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
F=$OUT/fresh
mkdir -p "$F"
PAGE() { python3 -I "$ROOT/e2e/page.py" "$@"; }
# t <name> <cozy args…>: one command as typed, in $DIR (else $F): its words in <name>.txt, its exit and seconds said.
t() {
  local n=$1 t0 rc; shift; t0=$(date +%s%N)
  (cd "${DIR:-$F}" && timeout "${TO:-1800}" cozy "$@") > "$F/$n.txt" 2>&1 < /dev/null; rc=$?
  say "$n: cozy $* -> exit $rc in $(( ($(date +%s%N) - t0) / 1000000 )) ms: $(grep -v '^\s*$' "$F/$n.txt" | tail -4 | tr '\n' ' ' | cut -c1-500)"
  return $rc
}
cfg=$HOME/.cozy/config.toml
say "cozy $(cozy --version); config: $(tr '\n' ' ' < "$cfg" 2> /dev/null | cut -c1-200)"
# mode dev (the coordinator, 11-18): a dev home from the first command, `local` on the RC's pair; the RC's packages from source (birefnet,
# qwen3 on a 0.8B model), then a PUBLISHED package locked on the released pair (birefnet@<its release>), then `machine update local`.
if [ "$(param mode published)" = dev ]; then
  SOURCE=$(cd "$PACKAGES/$(param source rc)" && pwd) || fail "no packages_refs checkout $(param source rc)"
  mkdir -p "${cfg%/*}"; { echo 'dev = true'; printf '\n[calcifer]\npackages = "%s"\n' "$SOURCE"; } > "$cfg"
  t restart-dev restart
  t doctor-fix doctor --fix -y
  t local-before machine show local
  t new new --unencrypted fresh
  P=$F/fresh; cp "$REF/seed.jpg" "$P/picture.jpg"; small=$P/small.jpg
  ffmpeg -v error -y -i "$P/picture.jpg" -vf scale=512:-2 "$small" 2> /dev/null || small=$P/picture.jpg
  DIR=$P t src-birefnet run birefnet/remove_background --file image="$small" --on local
  DIR=$P t src-write run qwen3/write instructions:='"Say hello in five words."' max_tokens=32 --model model=huihui-ai/huihui-qwen3.5-0.8b-abliterated --on local
  rel=$(cozy --json package ls 2> /dev/null < /dev/null | jq -r '[.. | objects | select((.name // .package) == "birefnet") | (.latest // .version)] | first // "0.9.0"' 2> /dev/null)
  DIR=$P t pub-birefnet run "birefnet/remove_background@${rel:-0.9.0}" --file image="$small" --on local
  DIR=$P t pub-birefnet-2 run "birefnet/remove_background@${rel:-0.9.0}" --file image="$small" --on local
  DIR=$P t update-local machine update local -y
  DIR=$P t local-after machine show local
  DIR=$P t pub-birefnet-after-update run "birefnet/remove_background@${rel:-0.9.0}" --file image="$small" --on local
  DIR=$P t src-birefnet-after run birefnet/remove_background --file image="$P/picture.jpg" --on local
  PAGE import "$P" "$small" --name pic > "$F/page-import.json" 2>&1
  PAGE add "$P" cut --tool birefnet/remove_background --fields '{"image": "<pic>"}' > "$F/page-add.json" 2>&1
  t0=$SECONDS; timeout 1800 python3 -I "$ROOT/e2e/page.py" render "$P" --on local cut > "$F/page-render.json" 2> "$F/page-render.err"
  say "page render on local (source birefnet): exit $? in $((SECONDS - t0)) s: $(jq -c '{failure: .asset.failure, versions: (.versions | length)}' "$F/page-render.json" 2> /dev/null | cut -c1-600) $(tail -c 300 "$F/page-render.err" | tr '\n' ' ')"
  DIR=$P t runs run ls
  say "fresh-user (dev) done"
  exit 0
fi
grep -qE '^(dev *= *true|packages *=)' "$cfg" 2> /dev/null && fail "this home has a dev or packages setting: $(cat "$cfg")"
t doctor-fix doctor --fix -y
t local-before machine show local
t new new --unencrypted fresh
P=$F/fresh
cp "$REF/seed.jpg" "$P/picture.jpg"
small=$P/small.jpg
ffmpeg -v error -y -i "$P/picture.jpg" -vf scale=512:-2 "$small" 2> /dev/null || small=$P/picture.jpg
try() {  # try <name> <cozy run args…>: the run; refused, `cozy machine update local` and once more
  local n=$1; shift
  DIR=$P t "$n" run "$@" --on local && return 0
  grep -aqiE 'machine update|update this machine|runtime|tensorfs|lock' "$F/$n.txt" || return 1
  DIR=$P t "$n-update" machine update local -y
  DIR=$P t local-after-update machine show local
  DIR=$P t "$n-again" run "$@" --on local
}
# The page first, as a page-only user meets it: a BiRefNet box on a picture, rendered on local before anything is updated.
PAGE import "$P" "$small" --name pic0 > "$F/page0-import.json" 2>&1
PAGE add "$P" cut0 --tool birefnet/remove_background --fields '{"image": "<pic0>"}' > "$F/page0-add.json" 2>&1
t0=$SECONDS; timeout 1800 python3 -I "$ROOT/e2e/page.py" render "$P" --on local cut0 > "$F/page0-render.json" 2> "$F/page0-render.err"
say "page render on local before any update: exit $? in $((SECONDS - t0)) s: $(jq -c '{failure: .asset.failure, versions: (.versions | length)}' "$F/page0-render.json" 2> /dev/null | cut -c1-600) $(tail -c 400 "$F/page0-render.err" | tr '\n' ' ')"
DIR=$P t local-after-page machine show local
try birefnet birefnet/remove_background --file image="$small"
try birefnet-2 birefnet/remove_background --file image="$P/picture.jpg"
DIR=$P t describe-write run qwen3/write --describe
try write qwen3/write instructions:='"Say hello in five words."' max_tokens=32 --model model=huihui-ai/huihui-qwen3.5-0.8b-abliterated
DIR=$P t runs run ls
# The page's way: a picture box and a BiRefNet box on it, rendered on local as the page asks.
PAGE import "$P" "$small" --name pic > "$F/page-import.json" 2>&1
PAGE add "$P" cut --tool birefnet/remove_background --fields '{"image": "<pic>"}' > "$F/page-add.json" 2>&1
t0=$SECONDS; timeout 1800 python3 -I "$ROOT/e2e/page.py" render "$P" --on local cut > "$F/page-render.json" 2> "$F/page-render.err"
say "page render on local: exit $? in $((SECONDS - t0)) s: $(jq -c '{failure: .asset.failure, versions: (.versions | length)}' "$F/page-render.json" 2> /dev/null | cut -c1-600) $(tail -c 400 "$F/page-render.err" | tr '\n' ' ')"
PAGE asset "$P" cut > "$F/page-asset.json" 2>&1
DIR=$P t local-end machine show local
if [ -n "$(param source '')" ]; then
  SOURCE=$(cd "$PACKAGES/$(param source)" && pwd) || fail "no packages_refs checkout $(param source)"
  { echo 'dev = true'; cat "$cfg"; printf '\n[calcifer]\npackages = "%s"\n' "$SOURCE"; } > "$cfg.new" && mv "$cfg.new" "$cfg"
  t restart-dev restart
  try src-birefnet birefnet/remove_background --file image="$small"
  DIR=$P t local-src machine show local
fi
say "fresh-user done"
