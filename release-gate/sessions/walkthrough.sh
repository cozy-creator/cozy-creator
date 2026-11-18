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
W=$OUT/walk SECRETS=${RUNNER_TEMP:-/tmp}/walk-secrets PJ=${RUNNER_TEMP:-/tmp}/walk-projects
mkdir -p "$W/cli" "$W/out" "$W/page" "$SECRETS" "$PJ"
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
  DIR=$PJ c new-enc new garden --passphrase-file "$SECRETS/pass.txt" --recovery-out "$SECRETS/recovery.txt"
  DIR=$PJ c new-plain new --unencrypted plain
  DIR=$PJ/garden c status-garden status
  DIR=$PJ/plain c status-plain status
  c vault-help vault --help
fi
[ -d "$PJ/plain" ] || DIR=$PJ c new-plain new --unencrypted plain
[ -d "$PJ/garden" ] || { head -c 24 /dev/urandom | base64 > "$SECRETS/pass.txt"; DIR=$PJ c new-enc new garden --passphrase-file "$SECRETS/pass.txt" --recovery-out "$SECRETS/recovery.txt"; }
# The tools a box can be, as the page lists them, and each one's fields.
PAGE tools "$PJ/plain" > "$W/tools.json" 2> "$W/tools.err" || say "page.py tools: $(tail -c 300 "$W/tools.err")"
say "tools listed: $(jq -r '[.tools[]?.id // .[]?.id] | length' "$W/tools.json" 2> /dev/null)"
for t in $(jq -r '[.tools[]?.id // .[]?.id] | .[]' "$W/tools.json" 2> /dev/null | grep -E 'qwen-image-2|klein|krea-2|sdxl|birefnet|seedvr2|h3-image|minimax-h3/|orbit' | sort -u); do
  DIR=$PJ/plain c "describe-${t//\//_}" run "$t" --describe
done

# 2. A first picture on this computer (its CPU): BiRefNet, the CLI's own example.
if has local; then
  DIR=$PJ/plain TO=2400 c local-birefnet run birefnet/remove_background --file image="$REF/seed.jpg" && say "local BiRefNet: $(tail -2 "$W/cli/local-birefnet.txt" | tr '\n' ' ')" ||
    say "local BiRefNet failed: $(tail -c 400 "$W/cli/local-birefnet.txt")"
  DIR=$PJ/plain c run-ls-after run ls
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
    WALK=$W WALK_PROJECT=$P WALK_STAGE=$1 WALK_ENGINE=$engine WALK_MACHINE=${NAME:-} WALK_PASS=$SECRETS/pass.txt FLOW_SPEC=walkthrough.spec.js COZY_VERSION=$V \
      HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder COZY=$(command -v cozy) CHROME=$(command -v google-chrome) OUT=$OUT/web-$1-$engine \
      NODE_PATH=$PW/node_modules node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/walkthrough.config.js" --project "$engine" \
      > "$W/page-$1-$engine.log" 2>&1
    say "page $1 in $engine: exit $? ($(grep -cE '^STEP-FAIL' "$W/page/$engine/$1-steps.txt" 2> /dev/null || echo ?) steps failed)"
  done
}
if has page; then
  DIR=$W c local-before-tour machine show local
  pageflow tour
  sleep 60
  DIR=$W c local-after-tour machine show local; DIR=$W c models-local-after-tour model ls --on local
  say "local after the page tour (a Qwen box made, no GPU here): $(grep -aE '^(disk use|running|queued|phase)' "$W/cli/local-after-tour.txt" | tr '\n' ' ' | cut -c1-300); before: $(grep -aE '^disk use' "$W/cli/local-before-tour.txt" | cut -c1-200)"
  cozy machine logs local -n 80 < /dev/null 2>&1 | grep -aiE 'download|prefetch|prepar|install|qwen' | tail -20 | cut -c1-250 | sed 's/^/    local: /' >> "$R"
fi

P=$PJ/$(param project garden)
# A tool by the id the palette lists it under (a dev home's may be namespaced).
tid() { jq -r --arg t "$1" '[(.tools // .)[]?.id] | map(select(. == $t or endswith("/" + $t))) | first // $t' "$W/tools.json" 2> /dev/null || echo "$1"; }
# media <name>: the box's newest take's file, kept in out/ and measured.
media() {
  local f; f=$(jq -r '.versions[-1].files // {} | to_entries[] | select(.key == "image" or .key == "video") | .value | (.path // .file // .local // empty)' "$W/out/$1.render.json" 2> /dev/null | tail -1)
  [ -z "$f" ] || [[ $f == /* ]] || f=$P/$f
  [ -n "$f" ] && [ -f "$f" ] || f=$(find "$P" -newer "$W/out/$1.render.err" -type f \( -name '*.png' -o -name '*.jpg' -o -name '*.webp' -o -name '*.mp4' \) -printf '%T@ %p\n' 2> /dev/null | grep -v '/\.vault-calcifer/' | sort -rn | head -1 | cut -d' ' -f2-)
  [ -n "$f" ] && [ -f "$f" ] || { echo "no file found"; return; }
  cp "$f" "$W/out/$1.${f##*.}"
  ffprobe -v error -show_entries stream=codec_type,width,height,duration,nb_frames -of compact "$f" 2> /dev/null | tr '\n' ' '
}
# 4. Rented: one machine by the CLI, the tools chained on it.
if has tools || has idle || has cancel || has seeds || has lost || has updates || has relock; then
  STEPS=$ROOT/$(param tools release-gate/sessions/fixtures/walkthrough/tools.json)
  for p in $(jq -r '[.[] | .tool // empty | split("/")[0]] | unique | .[]' "$STEPS"); do kernels "$SOURCE/$p"; done
  DIR=$W c rent-quote-text machine rent "$SKU" --disk-gb "$DISK"
  RENTING=$RENT
  quote "$(param seconds 3600)"
  t0=$SECONDS; order; ready; say "rented $NAME ($RID) in $((SECONDS - t0)) s by the CLI"
  DIR=$W c machine-ls-rented machine ls; DIR=$W c machine-show-rented machine show "$NAME"
fi
# Relock (W-D1; `cozy update` restarts Calcifer): in the ENCRYPTED project, a render going on the rental, then one on `local`, Calcifer
# restarted as an update does, the project unlocked again: does the take land, was the run canceled or did it finish, what does the box
# say while locked and after, is anything lost.
if has relock; then
  G=$PJ/garden
  PAGE import "$G" "$REF/seed.jpg" --name photo > "$W/out/garden-photo.json" 2>&1
  relock() {  # relock <label> <machine> <box> <tool> <fields>
    local label=$1 on=$2 box=$3 t0 i
    PAGE add "$G" "$box" --tool "$(tid "$4")" --fields "$5" > "$W/out/$box.add.json" 2>&1
    PAGE render "$G" --on "$on" --queue "$box" > "$W/out/$box.render.json" 2>&1
    for i in $(seq 1 300); do PAGE asset "$G" "$box" > "$W/out/$box-poll.json" 2> /dev/null; jq -e '.asset.rendering != null' "$W/out/$box-poll.json" > /dev/null 2>&1 && break; sleep 1; done
    say "relock $label: $box rendering on $on ($(jq -c '.asset.rendering' "$W/out/$box-poll.json" 2> /dev/null | cut -c1-120)); restarting Calcifer"
    t0=$SECONDS; DIR=$G c "restart-$label" restart
    DIR=$G c "status-locked-$label" status; PAGE asset "$G" "$box" > "$W/out/$box-locked.json" 2>&1
    DIR=$PJ/plain c "runs-while-locked-$label" run ls
    say "relock $label: after the restart: $(grep -aiE 'lock|health|error' "$W/cli/status-locked-$label.txt" | head -2 | tr '\n' ' ' | cut -c1-200); the box: $(head -c 200 "$W/out/$box-locked.json")"
    sleep 45
    DIR=$G c "unlock-$label" vault unlock --passphrase-file "$SECRETS/pass.txt"
    say "relock $label: unlocked $((SECONDS - t0)) s after the restart: exit $(tail -1 "$W/cli/times.tsv" | cut -f2)"
    for i in $(seq 1 120); do PAGE asset "$G" "$box" > "$W/out/$box-after.json" 2> /dev/null; jq -e '([.versions[]?] | length > 0) or (.asset.failure != null and (.asset.making | not))' "$W/out/$box-after.json" > /dev/null 2>&1 && break; sleep 5; done
    DIR=$G c "runs-after-$label" run ls
    say "relock $label: $((SECONDS - t0)) s after the restart the box has $(jq '[.versions[]?] | length' "$W/out/$box-after.json" 2> /dev/null) version(s), failure $(jq -c '.asset.failure' "$W/out/$box-after.json" 2> /dev/null | cut -c1-300); runs: $(sed -n 2,3p "$W/cli/runs-after-$label.txt" | tr '\n' ' ' | cut -c1-300)"
  }
  relock rental "$NAME" lockedsdxl sdxl/image '{"instructions": "a hot air balloon over a lavender field", "megapixels": 4, "seed": 21}'
  relock local local lockedcut birefnet/remove_background '{"image": "<photo>"}'
fi
# Updates (coordinator, 11-18: after `machine update` said completed, a render's first run waited 342 s; after a settings-only update the
# pod refused connections ~3 min): each update timed, `machine show` until it answers ready, then a small run's created → accepted →
# started; any gap over 5 s keeps the pod's own logs at once.
if has updates; then
  ms() { date +%s%3N; }
  upd() {
    local arm=$1 t0 t1 first='' rc run gap; shift
    t0=$(ms); (cd "$PJ/plain" && timeout 900 cozy machine update "$NAME" -y "$@") > "$W/upd-$arm.txt" 2>&1 < /dev/null; rc=$?; t1=$(ms)
    for _ in $(seq 1 900); do
      timeout 10 cozy machine show "$NAME" --json < /dev/null > "$W/upd-$arm-show.json" 2> /dev/null && jq -e '.state == "ready"' "$W/upd-$arm-show.json" > /dev/null 2>&1 && { first=$(ms); break; }
      sleep 1
    done
    (cd "$PJ/plain" && timeout 1800 cozy --json run sdxl/generate prompt="a red apple on a wooden table, $arm" seed=$RANDOM --on "$NAME") > "$W/upd-$arm-run.ndjson" 2>&1 < /dev/null
    run=$(handle "$W/upd-$arm-run.ndjson")
    (cd "$PJ/plain" && cozy --json run watch "$run" --full) > "$W/upd-$arm-record.json" 2>&1 < /dev/null
    local times; times=$(jq -c '[paths(scalars) as $p | select(($p[-1] | tostring) | test("_at_ms$")) | {key: ($p | map(tostring) | join(".")), value: getpath($p)}] | from_entries' "$W/upd-$arm-record.json" 2> /dev/null)
    gap=$(jq -r '((.accepted_at_ms // .["run.accepted_at_ms"] // 0) - (.created_at_ms // .["run.created_at_ms"] // 0))' <<< "$times" 2> /dev/null)
    say "update $arm ($*): exit $rc in $((t1 - t0)) ms ($(grep -v '^\s*$' "$W/upd-$arm.txt" | tail -2 | tr '\n' ' ' | cut -c1-200)); machine show ready $(( ${first:-$t1} - t1 )) ms after it returned$([ -n "$first" ] || echo ' (never answered ready in 15 min)'); run ${run:-?}: $times; created→accepted ${gap:-?} ms"
    if [ -z "$first" ] || [ "${gap:-0}" -gt 5000 ] 2> /dev/null || [ $(( ${first:-$t1} - t1 )) -gt 5000 ]; then
      cozy machine logs "$NAME" tensord -n 300 < /dev/null > "$W/upd-$arm-tensord.log" 2>&1
      cozy machine logs "$NAME" tensorfs -n 100 < /dev/null > "$W/upd-$arm-tensorfs.log" 2>&1
      cozy machine logs "$NAME" calcifer -n 150 < /dev/null > "$W/upd-$arm-calcifer.log" 2>&1
      say "update $arm: a gap over 5 s; the pod's tensord log's last 60 lines follow (upd-$arm-tensord.log)"
      tail -60 "$W/upd-$arm-tensord.log" | cut -c1-300 | sed 's/^/    /' >> "$R"
    fi
  }
  upd plain
  upd limit-off --gpu-memory-limit off
  upd limit-20 --gpu-memory-limit 20GiB
  upd limit-back --gpu-memory-limit off
fi
if has tools; then
  # The coordinator's check: an encrypted project's Qwen-Image 2 text-to-image box, shown as an open page holds it, prefetches only
  # expander_t2i on the machine it will render on.
  if jq -e 'any(.[]; .name == "sky")' "$STEPS" > /dev/null; then
    PAGE add "$P" sky --tool "$(tid qwen-image-2/image)" --fields '{"instructions": "a red kite over a green hill, morning light", "seed": 5}' > "$W/out/sky.add.json" 2>&1
    t0=$SECONDS; timeout 300 python3 -I "$ROOT/e2e/page.py" show "$P" > "$W/show-sky.json" 2>&1
    cozy model ls --on "$NAME" > "$W/prefetch-models.txt" 2>&1 < /dev/null
    cozy machine logs "$NAME" --since 15m -n 600 < /dev/null 2>&1 | grep -aiE 'expander|prefetch|prepar|download|install' | tail -80 > "$W/prefetch-log.txt"
    cp "$P/.vault-calcifer/diagnostics/walk.log" "$W/walk-garden.log" 2> /dev/null || find "$P" -name walk.log -exec cp {} "$W/walk-garden.log" \; 2> /dev/null
    grep -ahiE 'expander' "$W/prefetch-log.txt" "$W/walk-garden.log" "$W/prefetch-models.txt" 2> /dev/null | cut -c1-260 | head -20 | sed 's/^/    prefetch: /' >> "$R"
    say "prefetch, an encrypted project's t2i box shown $((SECONDS - t0)) s: $(grep -ahoiE 'expander_[a-z0-9_]+' "$W/prefetch-log.txt" "$W/prefetch-models.txt" "$W/walk-garden.log" 2> /dev/null | sort | uniq -c | tr '\n' ' ')"
  fi
  # Placeholders a step file may use: {klein_lora} (the catalog's first FLUX.3 Klein LoRA, as a CLI user finds one), {klein_model}.
  if grep -q '{klein_' "$STEPS"; then
    DIR=$P c lora-klein lora ls klein --limit 10
    (cd "$P" && cozy --json lora ls klein --limit 10) > "$W/lora-klein.json" 2>&1 < /dev/null
    KLEIN_LORA=$(jq -r '[.. | objects | select(((.family // "") | test("klein")) and ((.ref // .model // .name // "") | test("/"))) | (.ref // .model // .name)] | first // empty' "$W/lora-klein.json" 2> /dev/null)
    DIR=$P c describe-klein run flux.3-klein/image --describe
    KLEIN_MODEL=$(grep -aoE 'default [a-z0-9_.-]+/[a-z0-9_.-]+@[0-9.a-z]+' "$W/cli/describe-klein.txt" | head -1 | cut -d' ' -f2)
    say "Klein LoRA from the catalog: ${KLEIN_LORA:-none found}; Klein's base model: ${KLEIN_MODEL:-unknown}"
    sed -e "s#{klein_lora}#${KLEIN_LORA:-none/none}#g" -e "s#{klein_model}#${KLEIN_MODEL:-black-forest-labs/flux.3-klein}#g" "$STEPS" > "$W/steps.json"; STEPS=$W/steps.json
  fi
  while read -r s; do
    name=$(jq -r .name <<< "$s") tool=$(jq -r '.tool // ""' <<< "$s") kind=$(jq -r '.kind // "page"' <<< "$s")
    case $kind in
      import) PAGE import "$P" "$ROOT/$(jq -r .file <<< "$s")" --name "$name" > "$W/out/$name.add.json" 2>&1 || say "$name: import failed: $(tail -c 300 "$W/out/$name.add.json")"; continue ;;
      cli)
        mapfile -t args < <(jq -r '.args[]' <<< "$s" | sed "s#{ref}#$REF/seed.jpg#")
        DIR=$P TO=3600 c "cli-$name" run "$tool" --on "$NAME" "${args[@]}"; rc=$?
        say "cli $name (cozy run $tool): exit $rc in $(tail -1 "$W/cli/times.tsv" | cut -f3) ms: $(tail -3 "$W/cli/cli-$name.txt" | tr '\n' ' ' | cut -c1-300)"
        continue ;;
      cozy)
        mapfile -t args < <(jq -r '.args[]' <<< "$s" | sed "s#{machine}#$NAME#")
        DIR=$P TO=1800 c "$name" "${args[@]}"; rc=$?
        say "$name (cozy ${args[*]}): exit $rc in $(tail -1 "$W/cli/times.tsv" | cut -f3) ms: $(grep -v '^\s*$' "$W/cli/$name.txt" | tail -4 | tr '\n' ' ' | cut -c1-400)"
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
  if has gone; then
  # The client gone mid-run (a laptop's lid closed): a Krea render asked, this computer's Calcifer stopped for 2 min, started again.
  PAGE add "$P" lid --tool "$(tid krea-2/image)" --fields '{"instructions": "a lighthouse keeper reading by lamplight", "seed": 9}' > "$W/out/lid.add.json" 2>&1
  PAGE render "$P" --on "$NAME" --queue lid > "$W/out/lid.render.json" 2>&1; sleep 5
  unit=$(systemctl --user list-units --type=service --all --no-legend 2> /dev/null | awk '$1 ~ /(cozy|calcifer)/ && $1 ~ /\.service$/ {print $1; exit}')
  say "client gone: stopping ${unit:-no unit found} for 120 s"
  [ -z "$unit" ] || { systemctl --user stop "$unit"; sleep 120; systemctl --user start "$unit"; sleep 15; }
  t1=$SECONDS
  for _ in $(seq 1 60); do PAGE asset "$P" lid > "$W/out/lid.asset.json" 2>&1; jq -e '[.versions[]?] | length > 0' "$W/out/lid.asset.json" > /dev/null 2>&1 && break; sleep 10; done
  say "client gone: lid has $(jq '[.versions[]?] | length' "$W/out/lid.asset.json" 2> /dev/null) version(s) $((SECONDS - t1)) s after Calcifer came back"
  DIR=$P c status-after-gone status
  say "after Calcifer came back: $(grep -aiE 'lock|health' "$W/cli/status-after-gone.txt" | head -3 | tr '\n' ' ')"
  fi
  PAGE asset "$P" $(jq -r '.[] | select(.kind != "cli") | .name' "$STEPS") lid stop > "$W/assets.json" 2>&1
  DIR=$P c run-ls-tools run ls
  DIR=$W c machine-show-after machine show "$NAME"
  # Every run's full record, and what each failed one says about its credential.
  mkdir -p "$W/records"
  for r in $(cd "$P" && cozy --json run ls 2> /dev/null < /dev/null | jq -r '(.runs // .)[]? | (.name // .id)' 2> /dev/null | head -40); do
    (cd "$P" && cozy --json run watch "$r" --full) > "$W/records/$r.json" 2>&1 < /dev/null
  done
  for f in "$W"/records/*.json; do
    jq -e '(.status // .run.status) == "failed"' "$f" > /dev/null 2>&1 || continue
    say "record ${f##*/}: $(jq -c '{target: (.target // .run.target), reason: ((.reason // .run.reason) | tostring | .[0:220])}' "$f" 2> /dev/null); credential words: $(grep -aoiE '"[a-z_]*(sign|credential|token|grant|auth|bearer|account)[a-z_]*": *("[^"]{0,40}"|true|false|null|[0-9]+)' "$f" | sort -u | head -8 | tr '\n' ' ')"
  done
fi
# A Qwen render cancelled from the page near step 3, then the same box again (cozy-creator #3758: the executor kept warm): it completes,
# starts fast, its peak normal. A warm-up render first, so the cancel lands on a warm executor as a user's would.
if has cancel; then
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
    (cd "$P" && cozy --json run watch "$r" --full) > "$W/out/run-$r.json" 2>&1 < /dev/null
    say "run $r: $(jq -c '{status: (.status // .run.status), reason: ((.reason // .run.reason) | tostring | .[0:160]), created: (.created_at_ms // .run.created_at_ms), started: (.started_at_ms // .run.started_at_ms), settled: (.settled_at_ms // .run.settled_at_ms)}' "$W/out/run-$r.json" 2> /dev/null)"
  done
fi
# Seeds (cozy-creator #3772, Paul: "you can only memoize when you have a fixed seed"): a box with no seed rendered three times, three
# seeds and three pictures, none from memory; `cozy render <box> -n 3`, three seeds; a box at seed 42 twice, the second from memory.
if has seeds; then
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
{ has tools || has cancel || has seeds; } && [ -n "${NAME:-}" ] && pageflow results
# A machine lost mid-run: a long SDXL render going, the rental ended under it; what the box, the run and the CLI then say.
if has lost; then
  PAGE add "$P" lost --tool "$(tid sdxl/image)" --fields '{"instructions": "a sprawling city of glass towers at dusk, aerial view", "megapixels": 16, "seed": 4}' > "$W/out/lost.add.json" 2>&1
  PAGE render "$P" --on "$NAME" --queue lost > "$W/out/lost.render.json" 2>&1
  for _ in $(seq 1 300); do PAGE asset "$P" lost > "$W/out/lost-poll.json" 2> /dev/null; jq -e '.asset.rendering != null' "$W/out/lost-poll.json" > /dev/null 2>&1 && break; sleep 2; done
  sleep 5; t0=$SECONDS
  DIR=$W c machine-end-midrun machine end "$NAME" -y; say "machine ended mid-run: exit $? in $((SECONDS - t0)) s"
  for _ in $(seq 1 60); do PAGE asset "$P" lost > "$W/out/lost-after.json" 2> /dev/null; jq -e '.asset.failure != null and (.asset.making | not)' "$W/out/lost-after.json" > /dev/null 2>&1 && break; sleep 5; done
  say "lost: after $((SECONDS - t0)) s the box says $(jq -c '{failure: .asset.failure, making: .asset.making, rendering: .asset.rendering}' "$W/out/lost-after.json" 2> /dev/null | cut -c1-500)"
  DIR=$P c run-ls-lost run ls
  say "lost: run ls: $(sed -n 2p "$W/cli/run-ls-lost.txt" | cut -c1-300)"
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
[ ! -d "$PJ/garden" ] || [ -n "${KEEP_OPEN:-}" ] || DIR=$PJ c vault-lock-end vault --directory "$PJ/garden" lock
say "walkthrough done: $(wc -l < "$W/cli/times.tsv") commands, $(awk -F'\t' '$2 != 0' "$W/cli/times.tsv" | wc -l) non-zero"
cp "$W/cli/times.tsv" "$OUT/cli-times.tsv"
