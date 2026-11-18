# walkthrough (0.27.0 launch; Paul, 11-18: "the agent uses the actual app, and tries running the actual code, as it is intended to be used
# by a user"): a new user's session on this runner, in the RC's dev home as user-flow.sh sets it up. The CLI read and used as a new user
# would (every verb's --help, doctor, new, status, lists), a first project, the page in Chrome and WebKit (walkthrough.spec.js: each view a
# screenshot and its accessibility tree), a first picture on this computer; with `rent`, one machine rented by the CLI and the popular
# tools chained on it through the page's own requests (e2e/page.py), each result looked at, the page's view of each; its idle release; its
# end. Proof-branch harness (release/walkthrough-027), never merged. params:
#   tree     the packages_refs checkout the first-party packages run from (required)
#   phases   comma list of cli, page, local, tools, idle, update (default cli,page,local)
#   tools    a JSON file in this checkout, the tool steps (default release-gate/sessions/fixtures/walkthrough/tools.json)
#   seconds  what the cap must buy (default 3600)
#   engines  the page's browsers (default chrome,webkit)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WEB=$ROOT/e2e/cli/web
W=$OUT/walk SECRETS=${RUNNER_TEMP:-/tmp}/walk-secrets
mkdir -p "$W/cli" "$W/out" "$W/page" "$SECRETS"
PHASES=,$(param phases cli,page,local),
has() { [[ $PHASES == *",$1,"* ]]; }
TREE=$(param tree '')
for tool in jq node npm google-chrome ffprobe python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] || fail "dispatch with -f packages_refs=<name>=<packages RC> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
PAGE() { python3 -I "$ROOT/e2e/page.py" "$@"; }

# c <name> <cozy args…>: one command as a person types it, in $DIR (else $W): its output in cli/<name>.txt, its exit and time in cli/times.tsv.
c() {
  local n=$1 t0 rc; shift; t0=$(date +%s%N)
  (cd "${DIR:-$W}" && timeout "${TO:-900}" cozy "$@") > "$W/cli/$n.txt" 2>&1 < /dev/null; rc=$?
  printf '%s\t%s\t%s\tcozy %s\n' "$n" "$rc" "$(( ($(date +%s%N) - t0) / 1000000 ))" "$*" >> "$W/cli/times.tsv"
  return $rc
}
verbs() { awk '/^(Commands|COMMANDS|Subcommands):/{f=1;next} f && /^  +[a-z][a-z0-9-]*( |$)/{print $1; next} f && /^[^ ]/{f=0}' "$1" | grep -vx help; }

# The setup user-flow.sh makes: FUSE 3 and securefs (encrypted projects), the dev home on the packages checkout, diagnostics.
bash "$ROOT/e2e/cli/install-vault-driver.sh" fuse3 > "$OUT/fuse.log" 2>&1 || fail "no FUSE: $(tail -c 300 "$OUT/fuse.log")"
SECUREFS=$HOME/.cozy/tools/securefs/$(jq -r .version "$ROOT/crates/project-mount/helper/manifest.json")
if [ ! -x "$SECUREFS/securefs" ]; then
  archive=$(python3 -I "$ROOT/scripts/prepare-securefs.py" --target x86_64-unknown-linux-gnu --out-dir "$RUNNER_TEMP/securefs") || fail "no securefs"
  mkdir -p "$SECUREFS" && chmod 700 "$SECUREFS" &&
    python3 -I -c 'import sys, zipfile; open(sys.argv[2], "wb").write(zipfile.ZipFile(sys.argv[1]).read("securefs"))' "$archive" "$SECUREFS/securefs" &&
    chmod 755 "$SECUREFS/securefs" || fail "securefs not put in $SECUREFS"
fi
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build"
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg"; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "calcifer config not written"
c restart restart || fail "cozy restart: $(tail -c 300 "$W/cli/restart.txt")"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
trap 'code=$?; claim; (exit "$code"); finish' EXIT
say "walkthrough on cozy $V, a dev home, packages $TREE $(git -C "$SOURCE" rev-parse --short=8 HEAD); phases ${PHASES//,/ }"

# 1. The CLI as a new user reads it.
if has cli; then
  c version --version; c help --help
  for s in $(verbs "$W/cli/help.txt"); do
    c "help-$s" "$s" --help
    for ss in $(verbs "$W/cli/help-$s.txt"); do c "help-$s-$ss" "$s" "$ss" --help; done
  done
  c doctor doctor; c whoami whoami; c status-nowhere status; c machine-ls machine ls; c machine-show-local machine show local
  c model-ls model ls; c package-ls package ls; c run-ls run ls
  head -c 24 /dev/urandom | base64 > "$SECRETS/pass.txt"
  c new-notty new notty
  c new-enc new garden --passphrase-file "$SECRETS/pass.txt" --recovery-out "$SECRETS/recovery.txt"
  c new-plain new --unencrypted plain
  DIR=$W/garden c status-garden status
  DIR=$W/plain c status-plain status
  c vault-help vault --help
fi
[ -d "$W/plain" ] || c new-plain new --unencrypted plain
[ -d "$W/garden" ] || { head -c 24 /dev/urandom | base64 > "$SECRETS/pass.txt"; c new-enc new garden --passphrase-file "$SECRETS/pass.txt" --recovery-out "$SECRETS/recovery.txt"; }
# The tools a box can be, as the page lists them, and each one's fields.
PAGE tools "$W/plain" > "$W/tools.json" 2> "$W/tools.err" || say "page.py tools: $(tail -c 300 "$W/tools.err")"
say "tools listed: $(jq -r '[.tools[]?.id // .[]?.id] | length' "$W/tools.json" 2> /dev/null)"
for t in $(jq -r '[.tools[]?.id // .[]?.id] | .[]' "$W/tools.json" 2> /dev/null | grep -E 'qwen-image-2|klein|krea-2|sdxl|birefnet|seedvr2|h3-image|minimax-h3/|orbit' | sort -u); do
  DIR=$W/plain c "describe-${t//\//_}" run "$t" --describe
done

# 2. A first picture on this computer (its CPU): BiRefNet, the CLI's own example.
if has local; then
  DIR=$W/plain TO=2400 c local-birefnet run birefnet/remove_background --file image="$REF/seed.jpg" && say "local BiRefNet: $(tail -2 "$W/cli/local-birefnet.txt" | tr '\n' ' ')" ||
    say "local BiRefNet failed: $(tail -c 400 "$W/cli/local-birefnet.txt")"
  DIR=$W/plain c run-ls-after run ls
fi

# 3. The page.
PW=$OUT/playwright-test
if has page || has tools; then
  npm install -s --prefix "$PW" @playwright/test@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install @playwright/test: $(tail -c 300 "$OUT/npm.log")"
  [[ ,$(param engines chrome,webkit), != *,webkit,* ]] ||
    (cd "$PW" && sudo -E env "PATH=$PATH" node node_modules/playwright-core/cli.js install-deps webkit && node node_modules/playwright-core/cli.js install webkit) > "$OUT/webkit.log" 2>&1 ||
    say "webkit not installed: $(tail -c 300 "$OUT/webkit.log")"
fi
pageflow() {  # pageflow <stage>: walkthrough.spec.js's steps for that stage, in each engine
  local engine
  for engine in $(param engines chrome,webkit | tr , ' '); do
    [ "$engine" = chrome ] || [ -d "$HOME/.cache/ms-playwright" ] || continue
    WALK=$W WALK_STAGE=$1 WALK_ENGINE=$engine WALK_MACHINE=${NAME:-} WALK_PASS=$SECRETS/pass.txt FLOW_SPEC=walkthrough.spec.js COZY_VERSION=$V \
      HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder COZY=$(command -v cozy) CHROME=$(command -v google-chrome) OUT=$OUT/web-$1-$engine \
      NODE_PATH=$PW/node_modules node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/walkthrough.config.js" --project "$engine" \
      > "$W/page-$1-$engine.log" 2>&1
    say "page $1 in $engine: exit $? ($(grep -cE '^STEP-FAIL' "$W/page/$engine/$1-steps.txt" 2> /dev/null || echo ?) steps failed)"
  done
}
has page && pageflow tour

# 4. Rented: one machine by the CLI, the tools chained on it.
if has tools || has idle; then
  STEPS=$ROOT/$(param tools release-gate/sessions/fixtures/walkthrough/tools.json)
  for p in $(jq -r '[.[].tool | split("/")[0]] | unique | .[]' "$STEPS"); do kernels "$SOURCE/$p"; done
  DIR=$W c rent-quote-text machine rent "$SKU" --disk-gb "$DISK"
  RENTING=$RENT
  quote "$(param seconds 3600)"
  t0=$SECONDS; order; ready; say "rented $NAME ($RID) in $((SECONDS - t0)) s by the CLI"
  DIR=$W c machine-ls-rented machine ls; DIR=$W c machine-show-rented machine show "$NAME"
fi
if has tools; then
  P=$W/garden
  # The coordinator's check: an encrypted project's Qwen-Image 2 text-to-image box, shown as an open page holds it, prefetches only
  # expander_t2i on the machine it will render on.
  PAGE add "$P" sky --tool qwen-image-2/image --fields '{"prompt": "a red kite over a green hill, morning light"}' > "$W/add-sky.json" 2>&1
  timeout 240 python3 -I "$ROOT/e2e/page.py" show "$P" > "$W/show-sky.json" 2>&1
  cozy model ls --on "$NAME" --json > "$W/prefetch-models.json" 2>&1 < /dev/null
  cozy machine logs "$NAME" --since 10m -n 400 < /dev/null 2>&1 | grep -aiE 'expander|prefetch|prepare|download' | tail -60 > "$W/prefetch-log.txt"
  say "prefetch for an encrypted t2i box: $(grep -aoiE 'expander_[a-z0-9_]+' "$W/prefetch-log.txt" "$W/prefetch-models.json" 2> /dev/null | sort | uniq -c | tr '\n' ' ')"
  n=0
  while read -r s; do
    n=$((n + 1)); name=$(jq -r .name <<< "$s") tool=$(jq -r .tool <<< "$s") kind=$(jq -r '.kind // "page"' <<< "$s")
    if [ "$kind" = import ]; then PAGE import "$P" "$ROOT/$(jq -r .file <<< "$s")" --name "$name" > "$W/out/$name.add.json" 2>&1; continue; fi
    if [ "$kind" = cli ]; then
      DIR=$P TO=3600 c "cli-$name" run "$tool" --on "$NAME" $(jq -r '.args[]' <<< "$s"); say "cli $name ($tool): exit $? in $(tail -1 "$W/cli/times.tsv" | cut -f3) ms"
      continue
    fi
    PAGE add "$P" "$name" --tool "$tool" --fields "$(jq -c .fields <<< "$s")" > "$W/out/$name.add.json" 2>&1 || { say "$name: add failed: $(tail -c 300 "$W/out/$name.add.json")"; continue; }
    t1=$SECONDS
    timeout 3600 python3 -I "$ROOT/e2e/page.py" render "$P" --on "$NAME" "$name" > "$W/out/$name.render.json" 2> "$W/out/$name.render.err"
    rc=$?
    say "$name ($tool) on $NAME: exit $rc in $((SECONDS - t1)) s: $(tail -c 300 "$W/out/$name.render.json" | tr '\n' ' ') $(tail -c 200 "$W/out/$name.render.err" | tr '\n' ' ')"
  done < <(jq -c '.[]' "$STEPS")
  PAGE asset "$P" $(jq -r '.[] | select(.kind != "cli") | .name' "$STEPS") > "$W/assets.json" 2>&1
  DIR=$P c run-ls-tools run ls
  pageflow results
  DIR=$W c machine-show-after machine show "$NAME"
fi
# Its idle release: nothing asked of it for its 15 idle minutes, it ends by itself.
if has idle; then
  t0=$SECONDS
  while [ $((SECONDS - t0)) -lt 1500 ] && ! gone "$RID"; do sleep 30; done
  if gone "$RID"; then say "IDLE: $NAME released itself $((SECONDS - t0)) s after its last request"; else say "IDLE: $NAME still up after 25 idle min"; fi
  DIR=$W c machine-ls-idle machine ls
fi
if [ -n "${NAME:-}" ] && ! gone "$RID"; then
  t0=$SECONDS; DIR=$W c machine-end machine end "$NAME" -y; say "machine end $NAME: exit $?, $((SECONDS - t0)) s"
fi
if has update; then
  DIR=$W c update-check update --check; DIR=$W c update update; DIR=$W c version-after --version
fi
say "walkthrough done: $(wc -l < "$W/cli/times.tsv") commands, $(awk -F'\t' '$2 != 0' "$W/cli/times.tsv" | wc -l) non-zero"
cp "$W/cli/times.tsv" "$OUT/cli-times.tsv"
