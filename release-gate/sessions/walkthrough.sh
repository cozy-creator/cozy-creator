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
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
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
  c model-ls model ls; c package-ls package ls; c run-ls run ls; c lora-ls lora ls; c lora-search lora search sdxl; c model-search model search qwen
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

P=$W/garden
# A tool by the id the palette lists it under (a dev home's may be namespaced).
tid() { jq -r --arg t "$1" '[(.tools // .)[]?.id] | map(select(. == $t or endswith("/" + $t))) | first // $t' "$W/tools.json" 2> /dev/null || echo "$1"; }
# media <name>: the box's newest take's file, kept in out/ and measured.
media() {
  local f; f=$(jq -r '.versions[-1].files // {} | to_entries[] | select(.key == "image" or .key == "video") | .value | (.path // .file // .local // empty)' "$W/out/$1.render.json" 2> /dev/null | tail -1)
  [ -z "$f" ] || [[ $f == /* ]] || f=$P/$f
  [ -n "$f" ] && [ -f "$f" ] || f=$(find "$P" -newer "$W/out/$1.render.err" -type f \( -name '*.png' -o -name '*.jpg' -o -name '*.webp' -o -name '*.mp4' \) 2> /dev/null | grep -v '/\.' | head -1)
  [ -n "$f" ] && [ -f "$f" ] || { echo "no file found"; return; }
  cp "$f" "$W/out/$1.${f##*.}"
  ffprobe -v error -show_entries stream=codec_type,width,height,duration,nb_frames -of compact "$f" 2> /dev/null | tr '\n' ' '
}
# 4. Rented: one machine by the CLI, the tools chained on it.
if has tools || has idle || has cancel || has seeds; then
  STEPS=$ROOT/$(param tools release-gate/sessions/fixtures/walkthrough/tools.json)
  for p in $(jq -r '[.[] | .tool // empty | split("/")[0]] | unique | .[]' "$STEPS"); do kernels "$SOURCE/$p"; done
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
  PAGE add "$P" sky --tool "$(tid qwen-image-2/image)" --fields '{"instructions": "a red kite over a green hill, morning light", "seed": 5}' > "$W/out/sky.add.json" 2>&1
  t0=$SECONDS; timeout 300 python3 -I "$ROOT/e2e/page.py" show "$P" > "$W/show-sky.json" 2>&1
  cozy model ls --on "$NAME" > "$W/prefetch-models.txt" 2>&1 < /dev/null
  cozy machine logs "$NAME" --since 15m -n 600 < /dev/null 2>&1 | grep -aiE 'expander|prefetch|prepar|download|install' | tail -80 > "$W/prefetch-log.txt"
  cp "$P/.vault-calcifer/diagnostics/walk.log" "$W/walk-garden.log" 2> /dev/null || find "$P" -name walk.log -exec cp {} "$W/walk-garden.log" \; 2> /dev/null
  say "prefetch, an encrypted project's t2i box shown $((SECONDS - t0)) s: $(grep -ahoiE 'expander_[a-z0-9_]+' "$W/prefetch-log.txt" "$W/prefetch-models.txt" "$W/walk-garden.log" 2> /dev/null | sort | uniq -c | tr '\n' ' ')"
  while read -r s; do
    name=$(jq -r .name <<< "$s") tool=$(jq -r '.tool // ""' <<< "$s") kind=$(jq -r '.kind // "page"' <<< "$s")
    case $kind in
      import) PAGE import "$P" "$ROOT/$(jq -r .file <<< "$s")" --name "$name" > "$W/out/$name.add.json" 2>&1 || say "$name: import failed: $(tail -c 300 "$W/out/$name.add.json")"; continue ;;
      cli)
        mapfile -t args < <(jq -r '.args[]' <<< "$s" | sed "s#{ref}#$REF/seed.jpg#")
        DIR=$P TO=3600 c "cli-$name" run "$tool" --on "$NAME" "${args[@]}"; rc=$?
        say "cli $name (cozy run $tool): exit $rc in $(tail -1 "$W/cli/times.tsv" | cut -f3) ms: $(tail -3 "$W/cli/cli-$name.txt" | tr '\n' ' ' | cut -c1-300)"
        continue ;;
      page) PAGE add "$P" "$name" --tool "$(tid "$tool")" --fields "$(jq -c .fields <<< "$s")" > "$W/out/$name.add.json" 2>&1 || { say "$name: add failed: $(tail -c 300 "$W/out/$name.add.json")"; continue; } ;;
    esac
    t1=$SECONDS o=$name; [ ! -e "$W/out/$name.render.json" ] || o=$name-again
    timeout 3600 python3 -I "$ROOT/e2e/page.py" render "$P" --on "$NAME" "$name" > "$W/out/$o.render.json" 2> "$W/out/$o.render.err"; rc=$?
    say "$o (${tool:-its tool}): exit $rc in $((SECONDS - t1)) s; $(media "$o"); $(jq -r '.asset.failure // empty | tostring' "$W/out/$o.render.json" 2> /dev/null | cut -c1-300) $(tail -c 300 "$W/out/$o.render.err" | tr '\n' ' ')"
  done < <(jq -c '.[]' "$STEPS")
  # A cancelled run: an SDXL render asked, then cancelled as the page cancels it.
  PAGE add "$P" stop --tool "$(tid sdxl/image)" --fields '{"instructions": "a tall stack of pancakes", "megapixels": 4, "seed": 9}' > "$W/out/stop.add.json" 2>&1
  PAGE render "$P" --on "$NAME" --queue stop > "$W/out/stop.render.json" 2>&1; sleep 8
  PAGE cancel "$P" stop > "$W/out/stop.cancel.json" 2>&1; sleep 20
  DIR=$P c run-ls-cancel run ls
  say "cancel: $(tail -c 300 "$W/out/stop.cancel.json" | tr '\n' ' '); run ls: $(sed -n 2,3p "$W/cli/run-ls-cancel.txt" | tr '\n' ' ' | cut -c1-300)"
  # The client gone mid-run (a laptop's lid closed): a Krea render asked, this computer's Calcifer stopped for 2 min, started again.
  PAGE add "$P" lid --tool "$(tid krea-2/image)" --fields '{"instructions": "a lighthouse keeper reading by lamplight", "seed": 9}' > "$W/out/lid.add.json" 2>&1
  PAGE render "$P" --on "$NAME" --queue lid > "$W/out/lid.render.json" 2>&1; sleep 5
  unit=$(systemctl --user list-units --all --no-legend 2> /dev/null | awk '/cozy|calcifer/ {print $1; exit}')
  say "client gone: stopping ${unit:-no unit found} for 120 s"
  [ -z "$unit" ] || { systemctl --user stop "$unit"; sleep 120; systemctl --user start "$unit"; sleep 15; }
  t1=$SECONDS
  for _ in $(seq 1 60); do PAGE asset "$P" lid > "$W/out/lid.asset.json" 2>&1; jq -e '[.versions[]?] | length > 0' "$W/out/lid.asset.json" > /dev/null 2>&1 && break; sleep 10; done
  say "client gone: lid has $(jq '[.versions[]?] | length' "$W/out/lid.asset.json" 2> /dev/null) version(s) $((SECONDS - t1)) s after Calcifer came back"
  PAGE asset "$P" $(jq -r '.[] | select(.kind != "cli") | .name' "$STEPS") lid stop > "$W/assets.json" 2>&1
  DIR=$P c run-ls-tools run ls
  DIR=$W c machine-show-after machine show "$NAME"
  pageflow results
fi
# A Qwen render cancelled from the page near step 3, then the same box again (cozy-creator #3758: the executor kept warm): it completes,
# starts fast, its peak normal. A warm-up render first, so the cancel lands on a warm executor as a user's would.
if has cancel; then
  P=$W/garden
  PAGE add "$P" qr --tool "$(tid qwen-image-2/image)" --fields '{"instructions": "a stone bridge over a mountain river, autumn", "turbo": false, "megapixels": 1}' > "$W/out/qr.add.json" 2>&1
  t1=$SECONDS; timeout 3600 python3 -I "$ROOT/e2e/page.py" render "$P" --on "$NAME" qr > "$W/out/qr-warm.render.json" 2> "$W/out/qr-warm.render.err"
  say "qr warm-up: exit $? in $((SECONDS - t1)) s"
  t1=$SECONDS; PAGE render "$P" --on "$NAME" --queue qr > "$W/out/qr-cancel.render.json" 2>&1
  cancelled=''
  for _ in $(seq 1 600); do
    PAGE asset "$P" qr > "$W/out/qr-poll.json" 2> /dev/null
    jq -c --arg t $((SECONDS - t1)) '{t: $t, rendering: .asset.rendering, making: .asset.making}' "$W/out/qr-poll.json" >> "$W/out/qr-cancel.poll.ndjson" 2> /dev/null
    step=$(jq -r '.asset.rendering | if type == "object" then (.step // .done // .steps_done // 0) elif type == "number" then . else 0 end' "$W/out/qr-poll.json" 2> /dev/null)
    if [[ ${step%.*} =~ ^[0-9]+$ ]] && { [ "${step%.*}" -ge 3 ] || { (( $(awk -v s="$step" 'BEGIN{print (s > 0 && s < 1)}') )) && awk -v s="$step" 'BEGIN{exit !(s >= 0.075)}'; }; }; then
      PAGE cancel "$P" qr > "$W/out/qr.cancel.json" 2>&1; cancelled="at $((SECONDS - t1)) s, rendering $step"; break
    fi
    sleep 0.5
  done
  say "qr cancelled ${cancelled:-never (no step 3 seen; last: $(tail -1 "$W/out/qr-cancel.poll.ndjson" | cut -c1-200))}"
  sleep 10; DIR=$P c run-ls-after-cancel run ls
  t1=$SECONDS; timeout 3600 python3 -I "$ROOT/e2e/page.py" render "$P" --on "$NAME" qr > "$W/out/qr-after.render.json" 2> "$W/out/qr-after.render.err"; rc=$?
  say "qr again after the cancel: exit $rc in $((SECONDS - t1)) s; $(media qr-after) $(tail -c 300 "$W/out/qr-after.render.err" | tr '\n' ' ')"
  for r in $(cd "$P" && cozy --json run ls 2> /dev/null | jq -r '(.runs // .)[]? | (.name // .id)' | head -4); do
    (cd "$P" && cozy --json run show "$r" --full) > "$W/out/run-$r.json" 2>&1 < /dev/null
    say "run $r: $(jq -c '{status: (.status // .run.status), reason: ((.reason // .run.reason) | tostring | .[0:160]), created: (.created_at_ms // .run.created_at_ms), started: (.started_at_ms // .run.started_at_ms), settled: (.settled_at_ms // .run.settled_at_ms)}' "$W/out/run-$r.json" 2> /dev/null)"
  done
fi
# Seeds (cozy-creator #3772, Paul: "you can only memoize when you have a fixed seed"): a box with no seed rendered three times, three
# seeds and three pictures, none from memory; `cozy render <box> -n 3`, three seeds; a box at seed 42 twice, the second from memory.
if has seeds; then
  P=$W/garden
  PAGE add "$P" free --tool "$(tid sdxl/image)" --fields '{"instructions": "a teapot shaped like a snail, studio photo"}' > "$W/out/free.add.json" 2>&1
  for i in 1 2 3; do
    t1=$SECONDS; timeout 1800 python3 -I "$ROOT/e2e/page.py" render "$P" --on "$NAME" free > "$W/out/free-$i.render.json" 2> "$W/out/free-$i.render.err"
    say "free render $i: exit $? in $((SECONDS - t1)) s; $(media free-$i)"
  done
  DIR=$P TO=1800 c render-n3 render free -n 3 --new --on "$NAME"; say "cozy render free -n 3 --new: exit $?: $(tail -4 "$W/cli/render-n3.txt" | tr '\n' ' ' | cut -c1-300)"
  DIR=$P c render-help render --help
  PAGE add "$P" fixed --tool "$(tid sdxl/image)" --fields '{"instructions": "a teapot shaped like a snail, studio photo", "seed": 42}' > "$W/out/fixed.add.json" 2>&1
  for i in 1 2; do
    t1=$SECONDS; timeout 1800 python3 -I "$ROOT/e2e/page.py" render "$P" --on "$NAME" fixed > "$W/out/fixed-$i.render.json" 2> "$W/out/fixed-$i.render.err"
    say "fixed (seed 42) render $i: exit $? in $((SECONDS - t1)) s; $(jq -r '.asset.failure // empty | tostring' "$W/out/fixed-$i.render.json" 2> /dev/null | cut -c1-300) $(tail -c 300 "$W/out/fixed-$i.render.err" | tr '\n' ' ')"
  done
  PAGE asset "$P" free fixed > "$W/out/seeds-assets.json" 2>&1
  (cd "$P" && cozy --json run ls --full) > "$W/out/seeds-runs.json" 2>&1 < /dev/null
  # A LoRA, as a CLI user finds one: the catalog's first SDXL LoRA, on SDXL.
  DIR=$P c lora-sdxl lora ls --family sdxl --limit 5
  lora=$(cd "$P" && cozy --json lora ls --family sdxl --limit 5 2> /dev/null < /dev/null | jq -r '[.. | objects | select(has("ref") or has("model")) | (.ref // .model) | strings | select(test("^[a-z0-9_.-]+/[a-z0-9_.-]+"))] | first // empty' 2> /dev/null)
  if [ -n "$lora" ]; then
    DIR=$P TO=1800 c cli-sdxl-lora run sdxl/generate prompt='a cozy wooden cabin in a snowy forest at night' seed=5 --lora "$lora" --on "$NAME"
    say "cozy run sdxl/generate --lora $lora: exit $?: $(tail -3 "$W/cli/cli-sdxl-lora.txt" | tr '\n' ' ' | cut -c1-300)"
  else say "no SDXL LoRA found by cozy lora ls --family sdxl: $(head -c 300 "$W/cli/lora-sdxl.txt")"; fi
  say "seeds: $(jq -c '{name: .asset.name, versions: [.versions[]? | {v: (.version // .number // .id), seed: (.seed // .fields.seed // .request.seed // .call.seed // null), memo: (.memoized // .memo // null)}]}' "$W/out/seeds-assets.json" 2> /dev/null | tr '\n' ' ' | cut -c1-900)"
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
