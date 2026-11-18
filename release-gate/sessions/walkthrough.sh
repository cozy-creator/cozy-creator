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
#   lora, lora_words   the `lora` phase's LoRA (lilylilith/fontbox) and the words every one of its pictures is asked with
#   order_tries        lib.sh's: 1 here, an out-of-stock order is reported, never asked again
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
if has page || has tools || has zero || has cancel || has seeds || has lora || has flow; then
  npm install -s --prefix "$PW" @playwright/test@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install @playwright/test: $(tail -c 300 "$OUT/npm.log")"
  [[ ,$(param engines chrome,webkit), != *,webkit,* ]] ||
    (cd "$PW" && sudo -E env "PATH=$PATH" node node_modules/playwright-core/cli.js install-deps webkit && node node_modules/playwright-core/cli.js install webkit) > "$OUT/webkit.log" 2>&1 ||
    say "webkit not installed: $(tail -c 300 "$OUT/webkit.log")"
fi
pageflow() {  # pageflow <stage>: walkthrough.spec.js's steps for that stage, in each engine
  local engine
  for engine in $(param engines chrome,webkit | tr , ' '); do
    [ "$engine" = chrome ] || [ -d "$HOME/.cache/ms-playwright" ] || continue
    WALK=$W WALK_PROJECT=$P WALK_BOXES=${WALK_BOXES:-} WALK_STAGE=$1 WALK_ENGINE=$engine WALK_MACHINE=${NAME:-} WALK_PASS=$SECRETS/pass.txt FLOW_SPEC=walkthrough.spec.js COZY_VERSION=$V \
      HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder COZY=$(command -v cozy) CHROME=$(command -v google-chrome) OUT=$OUT/web-$1-$engine \
      NODE_PATH=$PW/node_modules node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/walkthrough.config.js" --project "$engine" \
      > "$W/page-$1-$engine.log" 2>&1
    say "page $1 in $engine: exit $? ($(grep -cE '^STEP-FAIL' "$W/page/$engine/$1-steps.txt" 2> /dev/null || true) steps failed)"
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
# going <project dir> <target regex>: the newest run of it that `cozy run ls` says is running, its whole line (status and progress).
going() { (cd "$1" && cozy run ls < /dev/null 2> /dev/null) | awk -v t="$2" 'NR > 1 && $2 ~ t && $4 ~ /^(in_progress|running)/ {print; exit}'; }
# stepof <run ls line>: the step it is at, from its progress (`3/40`, `step 3 of 40`), else 0.
stepof() { grep -oE '([0-9]+) ?(/|of) ?[0-9]+' <<< "$1" | head -1 | grep -oE '^[0-9]+' || echo 0; }
# pctof <run ls line>: how far it says it is, when it says so as a share (`drawing 40% · 24s left`), else 0.
pctof() { grep -oE '[0-9]+%' <<< "$1" | head -1 | tr -d % || echo 0; }
# Zero ($0, this computer's CPU; coordinator 11-18, cl-1271): Render on a box that has a take and unchanged inputs (BiRefNet, which takes
# no seed; a memoized tool at seed 42), seen in the page at 1, 5 and 30 s and by `cozy render`; how a take gets onto the disk from the
# page and where its seed reads; a cancel landing mid-run, by `cozy run ls`'s STATUS and PROGRESS, then the same box again.
if has zero; then
  Z=$PJ/plain ZT=$HERE/fixtures/walkthrough/zero.py
  # A fresh local has no Runtime pair of its own, and a script's `cozy-runtime>=…` locks the released one: given its pair first (W-S2).
  DIR=$Z c z-update-local machine update local -y
  small=$W/out/small.jpg; ffmpeg -v error -y -i "$REF/seed.jpg" -vf scale=512:-2 "$small" 2> /dev/null || cp "$REF/seed.jpg" "$small"
  PAGE import "$Z" "$small" --name photo > "$W/out/z-photo.json" 2>&1
  zr() {  # zr <box>: a page render on local, its time said
    local t1=$SECONDS; timeout 1800 python3 -I "$ROOT/e2e/page.py" render "$Z" --on local "$1" > "$W/out/z-$1-$2.render.json" 2> "$W/out/z-$1-$2.render.err"
    say "zero: $1 render $2 on local: exit $? in $((SECONDS - t1)) s; versions $(jq '.versions | length' "$W/out/z-$1-$2.render.json" 2> /dev/null); $(tail -c 200 "$W/out/z-$1-$2.render.err" | tr '\n' ' ')"
  }
  PAGE add "$Z" cut --tool birefnet/remove_background --fields '{"image": "<photo>"}' > "$W/out/z-cut.add.json" 2>&1; zr cut 1
  PAGE add "$Z" fixedbox --tool "$ZT/seeded" --fields '{"instructions": "a fixed seed", "seed": 42}' > "$W/out/z-fixed.add.json" 2>&1; zr fixedbox 1
  PAGE add "$Z" freebox --tool "$ZT/seeded" --fields '{"instructions": "no seed"}' > "$W/out/z-free.add.json" 2>&1; zr freebox 1; zr freebox 2
  DIR=$Z c z-runs-before run ls
  WALK_BOXES=cut,fixedbox pageflow rerender
  DIR=$Z c z-runs-after-page run ls; DIR=$Z c z-runs-after-page-json --json run ls --full
  PAGE board "$Z" > "$W/out/z-board.json" 2>&1
  say "zero: the board's jobs after the page's Render: $(jq -c '[(.jobs // .board.jobs // [])[] | {asset, state, number, why, run}]' "$W/out/z-board.json" 2> /dev/null | cut -c1-600)"
  DIR=$Z c z-render-cut render cut --on local; DIR=$Z c z-render-fixed render fixedbox --on local; DIR=$Z c z-render-free render freebox --on local
  for b in cut fixedbox freebox; do say "zero: cozy render $b: $(grep -v '^\s*$' "$W/cli/z-render-${b%box}.txt" 2> /dev/null | head -3 | tr '\n' ' ' | cut -c1-300)"; done
  DIR=$Z c z-runs-after-cli run ls
  sed -n 1,12p "$W/cli/z-runs-after-cli.txt" | cut -c1-200 | sed 's/^/    zero runs: /' >> "$R"
  r=$(awk 'NR > 1 && $2 ~ /seeded/ {print $1; exit}' "$W/cli/z-runs-after-cli.txt")
  [ -z "$r" ] || { DIR=$Z c z-watch-seeded --json run watch "$r" --full; say "zero: run $r's record names its seed: $(grep -aoE '"seed": ?[0-9]+' "$W/cli/z-watch-seeded.txt" | sort -u | head -3 | tr '\n' ' ')"; }
  # A cancel mid-run: the slow still asked, cancelled as the page cancels it once `cozy run ls` says it's at step 3, then asked again.
  PAGE add "$Z" slowbox --tool "$ZT/slow" --fields '{"instructions": "slowly", "seconds": 40}' > "$W/out/z-slow.add.json" 2>&1
  t1=$SECONDS; PAGE render "$Z" --on local --queue slowbox > "$W/out/z-slow.queue.json" 2>&1
  cancelled='' first=''
  for _ in $(seq 1 600); do
    line=$(going "$Z" 'slow'); printf '%s\t%s\n' "$((SECONDS - t1))" "$line" >> "$W/out/z-slow.poll.tsv"
    if [ -n "$line" ]; then
      first=${first:-$SECONDS}; step=$(stepof "$line")
      if [ "${step:-0}" -ge 3 ] || [ $((SECONDS - first)) -ge 12 ]; then
        PAGE cancel "$Z" slowbox > "$W/out/z-slow.cancel.json" 2>&1; cancelled="at $((SECONDS - t1)) s, step ${step:-?} ($line)"; break
      fi
    fi
    sleep 0.5
  done
  sleep 8; DIR=$Z c z-runs-cancel run ls
  say "zero: slowbox cancelled ${cancelled:-never (never seen running)}; then: $(awk 'NR > 1 && $2 ~ /slow/ {print; exit}' "$W/cli/z-runs-cancel.txt" | tr -s ' ' | cut -c1-200)"
  zr slowbox after-cancel
  DIR=$Z c z-runs-final run ls
  say "zero: slowbox again: $(awk 'NR > 1 && $2 ~ /slow/ {print; exit}' "$W/cli/z-runs-final.txt" | tr -s ' ' | cut -c1-200)"
fi
ms() { date +%s%3N; }
# The LoRA picture pair (cl-1241; the coordinator, 11-18): a LoRA naming no slot (`cozy run --lora <lora>`, one dropped on a box) on FLUX.3
# Klein's, Fibo's and Krea 2's `image` went on the text encoder and was refused; it now goes on the box's models, Base and distilled, and
# the one the request loads takes it. At $0, before anything is rented: what the catalog says of the LoRA and of each family's, what each
# tool declares, and two Klein boxes written as a drop writes one (the box's own `lora`, no slot; its trigger words after its words) and
# as the page shows them.
LORA=$(param lora lilylilith/fontbox)
lora_look() {
  DIR=$P c lora-show lora show "$LORA"
  (cd "$P" && cozy --json lora show "$LORA") > "$W/lora-show.json" 2>&1 < /dev/null
  (cd "$P" && cozy --json lora ls "${LORA##*/}" --limit 5) > "$W/lora-card.json" 2>&1 < /dev/null
  TRIGGERS=$(jq -r --arg l "$LORA" '[.. | objects | select((.model // "") == $l) | .triggers // empty | .[]] | unique | join(", ")' "$W/lora-card.json" "$W/lora-show.json" 2> /dev/null | grep -v '^$' | head -1)
  WORDS=$(param lora_words 'a hand-painted wooden shop sign that reads "COZY BAKERY" in large ornate letters, hanging over a cobblestone street, morning light')
  [ -z "$TRIGGERS" ] || WORDS="$WORDS, $TRIGGERS"
  say "lora: $LORA, its card's triggers: ${TRIGGERS:-none}; the words: $WORDS"
  local t
  for t in flux.3-klein fibo-2 krea-2; do
    DIR=$P c "describe-$t" run "$t/image" --describe
    say "lora: $t/image's models: $(awk '/^  models:/{f=1;next} f && /^  [a-z]/{f=0} f' "$W/cli/describe-$t.txt" | tr -s ' ' | tr '\n' ';' | cut -c1-600)"
  done
  # Every LoRA the Hub lists, by family: what a family's users can pick from.
  : > "$W/lora-all.ndjson"
  local off
  for off in 0 100 200 300 400; do
    (cd "$P" && cozy --json lora ls --limit 100 --offset "$off") > "$W/lora-page.json" 2>> "$W/lora-all.err" < /dev/null
    jq -c '.models[]?' "$W/lora-page.json" >> "$W/lora-all.ndjson" 2> /dev/null
    [ "$(jq '.models | length' "$W/lora-page.json" 2> /dev/null || echo 0)" = 100 ] || break
  done
  say "lora: the Hub's LoRAs by family: $(jq -rs 'group_by(.family) | map("\(.[0].family // "none") \(length)") | join(", ")' "$W/lora-all.ndjson" 2> /dev/null)"
  for t in fibo krea; do
    say "lora: $t LoRAs listed: $(jq -rs --arg f "$t" '[.[] | select((.family // "") | test($f)) | .model] | join(" ")' "$W/lora-all.ndjson" 2> /dev/null | cut -c1-400)"
  done
  # Seed 11, not the CLI pairs' 7: the same request as a CLI run's would be answered from memory, and these are to run.
  jq -cn --arg w "$WORDS" --arg l "$LORA" '{instructions: $w, seed: 11, lora: [{ref: $l}]}' > "$W/out/kdrop.fields.json"
  jq -cn --arg w "$WORDS" '{instructions: $w, seed: 11}' > "$W/out/kplain.fields.json"
  PAGE add "$P" kdrop --tool "$(tid flux.3-klein/image)" --fields "$(cat "$W/out/kdrop.fields.json")" > "$W/out/kdrop.add.json" 2>&1 || say "kdrop: add failed: $(tail -c 300 "$W/out/kdrop.add.json")"
  PAGE add "$P" kplain --tool "$(tid flux.3-klein/image)" --fields "$(cat "$W/out/kplain.fields.json")" > "$W/out/kplain.add.json" 2>&1 || say "kplain: add failed: $(tail -c 300 "$W/out/kplain.add.json")"
  PAGE asset "$P" kdrop kplain > "$W/out/lora-boxes.json" 2>&1
  say "lora: the boxes as written: $(jq -c '{name: .asset.name, tool: .asset.tool, fields: .asset.fields}' "$W/out/lora-boxes.json" 2> /dev/null | tr '\n' ' ' | cut -c1-700)"
  WALK_BOXES=kdrop,kplain pageflow lora-look
}
if has lora; then
  [ -d "$PJ/$(param project garden)" ] || fail "no project $(param project garden)"
  lora_look
fi
# 4. Rented: one machine by the CLI, the tools chained on it.
if has tools || has idle || has cancel || has seeds || has lost || has updates || has relock || has predict || has lora || has cold; then
  STEPS=$ROOT/$(param tools release-gate/sessions/fixtures/walkthrough/tools.json)
  for p in $(jq -r '[.[] | .tool // empty | split("/")[0]] | unique | .[]' "$STEPS"); do kernels "$SOURCE/$p"; done
  ! { has lora || has cold; } || for p in flux.3-klein fibo-2 krea-2; do kernels "$SOURCE/$p"; done
  DIR=$W c rent-quote-text machine rent "$SKU" --disk-gb "$DISK"
  RENTING=$RENT
  quote "$(param seconds 3600)"
  t0=$SECONDS; order; ready; say "rented $NAME ($RID) in $((SECONDS - t0)) s by the CLI"
  DIR=$W c machine-ls-rented machine ls; DIR=$W c machine-show-rented machine show "$NAME"
fi
# Predict (#3932): the rental ready, a Qwen Image 2.1 box and a Klein box made and their words written, the project shown as an open
# page holds it, nothing rendered: within about a minute the rental installs the packages and fetches their models ahead, and `local`
# (this GPU-less runner) receives nothing; then Render starts without a fresh download. Each 10 s: what the pod and local hold.
phase_predict() {
  ms() { date +%s%3N; }
  # holds <machine>: each model it holds, `name held/own`: `model ls`'s size is bytes HELD, its `shared` the part another model shares, so a
  # slot's own bytes are held less shared (the coordinator, 11-18: two enhancers' 889 MiB were all shared, never either one whole).
  holds() { cozy model ls --on "$1" < /dev/null 2> /dev/null | awk -v m="$1" '
    function bytes(t,   n, u) { n = t + 0; u = t; sub(/^[0-9.]+ /, "", u); return n * (u == "GiB" ? 2^30 : u == "MiB" ? 2^20 : u == "KiB" ? 2^10 : 1) }
    function human(b) { return b >= 2^30 ? sprintf("%.1fG", b / 2^30) : b >= 2^20 ? sprintf("%.0fM", b / 2^20) : sprintf("%.0fK", b / 2^10) }
    NR > 1 && $1 == m { line = $0; n = 0
      while (match(line, /[0-9.]+ (B|KiB|MiB|GiB)/)) { v[++n] = substr(line, RSTART, RLENGTH); line = substr(line, RSTART + RLENGTH) }
      held = n >= 1 ? bytes(v[1]) : 0; shared = n >= 2 ? bytes(v[2]) : 0
      print $2 " " human(held) "/" human(held - shared) }' | sort | paste -sd, -; }
  PAGE import "$P" "$REF/seed.jpg" --name photo > "$W/out/predict-photo.json" 2>&1
  DIR=$P c predict-local-before model ls --on local; DIR=$P c predict-pod-before model ls --on "$NAME"
  t0=$(ms)
  PAGE add "$P" pq --tool "$(tid qwen-image-2/image)" --fields '{"instructions": "a paper lantern festival on a river at night, wide shot"}' > "$W/out/pq.add.json" 2>&1
  PAGE add "$P" pk --tool "$(tid flux.3-klein/image)" --fields '{"instructions": "a paper lantern festival on a river at night, oil painting"}' > "$W/out/pk.add.json" 2>&1
  say "predict: boxes pq (Qwen Image 2.1) and pk (Klein) written, nothing rendered; the project shown as an open page holds it"
  ( timeout 330 python3 -I "$ROOT/e2e/page.py" show "$P" > "$W/predict-show.json" 2>&1 ) & shown=$!
  local i edited=''
  for i in $(seq 1 30); do
    sleep 10
    # An edit that cuts a download: at +120 s pq comes to mention a picture, so its text-to-image enhancer is no longer what it needs.
    if [ "$i" = 12 ]; then
      PAGE set "$P" pq --fields '{"instructions": "<photo> as a paper lantern festival on a river at night"}' > "$W/out/pq.set.json" 2>&1
      edited=$(( ($(ms) - t0) / 1000 )); say "predict: pq edited at +${edited}s to mention <photo> (its i2i enhancer now, not t2i)"
    fi
    printf '%s\t+%ss\tpod: %s\tlocal: %s\tpod: %s\n' "$(date -u +%T)" $(( ($(ms) - t0) / 1000 )) "$(holds "$NAME")" "$(holds local)" \
      "$(cozy machine show "$NAME" < /dev/null 2> /dev/null | grep -aiE '^(running|queued|activity|installing|downloading|disk use)' | tr -s ' ' | tr '\n' ';' | cut -c1-240)" >> "$W/predict.tsv"
  done
  wait "$shown" 2> /dev/null
  cut -c1-520 "$W/predict.tsv" | sed 's/^/    predict: /' >> "$R"
  cozy machine logs "$NAME" tensorfs --since 8m -n 400 < /dev/null 2>&1 | grep -aiE 'refused|nothing wants|cancel|cut|landed|pe-t2i|pe-i2i|expander' | tail -40 | cut -c1-260 > "$W/predict-tensorfs.log"
  sed 's/^/    predict tensorfs: /' "$W/predict-tensorfs.log" | tail -25 >> "$R"
  DIR=$P c predict-local-after model ls --on local; DIR=$P c predict-pod-after model ls --on "$NAME"
  say "predict: local after: $(holds local | tr ',' '\n' | grep -c .) model(s) (before: $(awk 'NR > 1 && $1 == "local"' "$W/cli/predict-local-before.txt" | wc -l))"
  local b t1 rc r
  for b in pq pk; do
    t1=$(ms); timeout 1800 python3 -I "$ROOT/e2e/page.py" render "$P" --on "$NAME" "$b" > "$W/out/$b.render.json" 2> "$W/out/$b.render.err"; rc=$?
    r=$(cd "$P" && cozy run ls < /dev/null 2> /dev/null | awk -v t="$([ "$b" = pq ] && echo qwen-image-2/image || echo flux.3-klein/image)" 'NR > 1 && $2 ~ t {print $1; exit}')
    [ -z "$r" ] || (cd "$P" && cozy --json run watch "$r" --full) > "$W/out/$b-watch.ndjson" 2>&1 < /dev/null
    # What it downloaded on its critical path is its record's `measurements.weights` (`needs`), never a count of words in this file: a
    # watch of a settled run holds only its final record. Its install, a run of its own just before it: how long from accepted to settled.
    local need inst
    need=$(grep -a '^{' "$W/out/$b-watch.ndjson" | tail -1 | jq -r '[.. | objects | select(has("measurements")) | .measurements.weights // empty][0] // {} | "had \((.has // 0) / 1e9 | . * 10 | round / 10) GB, downloaded \((.needs // 0) / 1e9 | . * 10 | round / 10) GB on its way, extras \((.extras // 0) / 1e9 | . * 10 | round / 10) GB"' 2> /dev/null)
    inst=$(cd "$P" && cozy run ls < /dev/null 2> /dev/null | awk -v t="$([ "$b" = pq ] && echo qwen-image-2 || echo flux.3-klein)" 'NR > 1 && $0 ~ "install local/" t {print $1; exit}')
    [ -z "$inst" ] || inst="$inst $(cd "$P" && cozy --json run watch "$inst" --full < /dev/null 2> /dev/null | grep -a '^{' | tail -1 | jq -r '[.. | objects | select(has("accepted_at_ms") and has("settled_at_ms"))][0] | "settled \(.settled_at_ms - .accepted_at_ms) ms after accepted"' 2> /dev/null)"
    say "predict: $b rendered after the guess: exit $rc in $(( ($(ms) - t1) / 1000 )) s, run ${r:-?}: ${need:-no weights measurement}; its install ${inst:-not found}; $(tail -c 200 "$W/out/$b.render.err" | tr '\n' ' ')"
  done
  DIR=$P c predict-runs run ls
  sed -n 2,5p "$W/cli/predict-runs.txt" | cut -c1-300 | sed 's/^/    predict: /' >> "$R"
  DIR=$P c predict-pod-final model ls --on "$NAME"
}
# Relock (W-D1; `cozy update` restarts Calcifer): in the ENCRYPTED project, a render going on the rental, then one on `local`, Calcifer
# restarted as an update does, the project unlocked again: does the take land, was the run canceled or did it finish, what does the box
# say while locked and after, is anything lost.
phase_relock() {
  G=$PJ/garden
  PAGE import "$G" "$REF/seed.jpg" --name photo > "$W/out/garden-photo.json" 2>&1
  relock() {  # relock <label> <machine> <box> <tool> <fields>
    local label=$1 on=$2 box=$3 t0 i
    PAGE add "$G" "$box" --tool "$(tid "$4")" --fields "$5" > "$W/out/$box.add.json" 2>&1
    PAGE render "$G" --on "$on" --queue "$box" > "$W/out/$box.render.json" 2>&1
    local line=''
    for i in $(seq 1 600); do line=$(going "$G" "${4%%@*}"); [ -z "$line" ] || break; sleep 1; done
    sleep 3
    say "relock $label: $box running on $on (${line:-never seen running}); restarting Calcifer"
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
}
# Updates (coordinator, 11-18: after `machine update` said completed, a render's first run waited 342 s; after a settings-only update the
# pod refused connections ~3 min): each update timed, `machine show` until it answers ready, then a small run's created → accepted →
# started; any gap over 5 s keeps the pod's own logs at once.
phase_updates() {
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
}
phase_tools() {
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
    KLEIN_MODEL=$(awk '$1 == "model:" {for (i = 1; i <= NF; i++) if ($i == "(default") {sub(/\)$/, "", $(i + 1)); print $(i + 1); exit}}' "$W/cli/describe-klein.txt")
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
}
# A Qwen render cancelled from the page near step 3, then the same box again (cozy-creator #3758: the executor kept warm): it completes,
# starts fast, its peak normal. A warm-up render first, so the cancel lands on a warm executor as a user's would.
phase_cancel() {
  local CT CF; CT=$(param cancel_tool qwen-image-2/image) CF=$(param cancel_fields '{"instructions": "a stone bridge over a mountain river, autumn", "turbo": false, "megapixels": 1}')
  PAGE add "$P" qr --tool "$(tid "$CT")" --fields "$CF" > "$W/out/qr.add.json" 2>&1
  t1=$SECONDS; timeout 3600 python3 -I "$ROOT/e2e/page.py" render "$P" --on "$NAME" qr > "$W/out/qr-warm.render.json" 2> "$W/out/qr-warm.render.err"
  say "qr warm-up: exit $? in $((SECONDS - t1)) s"
  t1=$SECONDS; PAGE render "$P" --on "$NAME" --queue qr > "$W/out/qr-cancel.render.json" 2>&1
  cancelled=''
  first=''
  for _ in $(seq 1 1200); do
    line=$(going "$P" "$CT")
    printf '%s\t%s\n' "$((SECONDS - t1))" "$line" >> "$W/out/qr-cancel.poll.tsv"
    if [ -n "$line" ]; then
      first=${first:-$SECONDS}; step=$(stepof "$line")
      if [ "${step:-0}" -ge 3 ] || [ "$(pctof "$line")" -ge 8 ] 2> /dev/null || [ $((SECONDS - first)) -ge 20 ]; then
        PAGE cancel "$P" qr > "$W/out/qr.cancel.json" 2>&1; cancelled="at $((SECONDS - t1)) s, at step ${step:-?} ($line)"; break
      fi
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
}
# Seeds (cozy-creator #3772, Paul: "you can only memoize when you have a fixed seed"): a box with no seed rendered three times, three
# seeds and three pictures, none from memory; `cozy render <box> -n 3`, three seeds; a box at seed 42 twice, the second from memory.
phase_seeds() {
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
}
# A machine lost mid-run: a long SDXL render going, the rental ended under it; what the box, the run and the CLI then say.
phase_lost() {
  PAGE add "$P" lost --tool "$(tid sdxl/image)" --fields '{"instructions": "a sprawling city of glass towers at dusk, aerial view", "megapixels": 16, "seed": 4}' > "$W/out/lost.add.json" 2>&1
  PAGE render "$P" --on "$NAME" --queue lost > "$W/out/lost.render.json" 2>&1
  for _ in $(seq 1 600); do line=$(going "$P" 'sdxl/image'); [ -z "$line" ] || break; sleep 1; done
  say "lost: the render is going: ${line:-never seen running}"
  sleep 5; t0=$SECONDS
  DIR=$W c machine-end-midrun machine end "$NAME" -y; say "machine ended mid-run: exit $? in $((SECONDS - t0)) s"
  for _ in $(seq 1 60); do PAGE asset "$P" lost > "$W/out/lost-after.json" 2> /dev/null; jq -e '.asset.failure != null and (.asset.making | not)' "$W/out/lost-after.json" > /dev/null 2>&1 && break; sleep 5; done
  say "lost: after $((SECONDS - t0)) s the box says $(jq -c '{failure: .asset.failure, making: .asset.making, rendering: .asset.rendering}' "$W/out/lost-after.json" 2> /dev/null | cut -c1-500)"
  DIR=$P c run-ls-lost run ls
  say "lost: run ls: $(sed -n 2p "$W/cli/run-ls-lost.txt" | cut -c1-300)"
}
# left: the seconds this session still has before its step's deadline (5 min kept for the end).
left() { echo $(( ${SESSION_UNTIL:-$(($(date +%s) + 7200))} - $(date +%s) - 300 )); }
# lrun <name> <tool> <args…>: one `cozy run` on the rental as a person types it, its pictures kept in out/<name>/, what it printed in
# out/<name>.txt, its record in records/<name>.json; said in one line: where its LoRA went, what loaded, its times.
newest() { (cd "$P" && cozy --json run ls --limit 1 2> /dev/null < /dev/null) | jq -r '(.runs // .)[0] | (.name // .id) // empty' 2> /dev/null; }
lrun() {
  local n=$1 tool=$2 t0 rc run before after p t=0 slow=''; shift 2
  mkdir -p "$W/out/$n" "$W/records"
  before=$(newest); t0=$(ms)
  (cd "$P" && exec timeout "${TO:-2400}" cozy run "$tool" "$@" --on "$NAME" -o "$W/out/$n") > "$W/out/$n.txt" 2>&1 < /dev/null & p=$!
  while kill -0 "$p" 2> /dev/null; do
    sleep 1; t=$((t + 1))
    # lib.sh's floor: a weights download still under FLOOR_MBS once the Hub has the host's speed report ends the session.
    if [ "$FLOOR_MBS" != 0 ] && [ "$t" -ge "$FLOOR_AFTER" ] && [ $((t % 15)) = 0 ] && slow=$(floor "$W/out/$n.txt" downloading) && heard "$slow"; then
      kill "$p"; fail "host too slow: $slow on $NAME ($RID): $HEARD"
    fi
  done
  wait "$p"; rc=$?
  printf '%s\t%s\t%s\tcozy run %s %s --on %s\n' "$n" "$rc" "$(( $(ms) - t0 ))" "$tool" "$*" "$NAME" >> "$W/cli/times.tsv"
  # Its run: the one its own output names; else one newer than the newest before it. A call refused before it was a run has none.
  run=$(awk '$1=="run" && NF==2{print $2}' "$W/out/$n.txt" | tail -1)
  if [ -z "$run" ]; then after=$(newest); [ "$after" = "$before" ] || run=$after; fi
  : > "$W/records/$n.json"
  [ -z "$run" ] || (cd "$P" && cozy --json run watch "$run" --once --full) > "$W/records/$n.raw" 2>&1 < /dev/null
  [ -z "$run" ] || grep -a '^{' "$W/records/$n.raw" 2> /dev/null | tail -1 > "$W/records/$n.json"
  say "$n: cozy run $tool $* --on $NAME: exit $rc in $(( $(ms) - t0 )) ms, run ${run:-none (no run made)}: $(lrec "$n")"
  [ "$rc" = 0 ] || { say "$n NOT COMPLETED (exit $rc), its last lines:"; grep -v '^\s*$' "$W/out/$n.txt" | tail -8 | cut -c1-500 | sed 's/^/    /' | tee -a "$R"; }
  return "$rc"
}
# lrec <name>: what a run's own record says: its status, the slot its LoRA went on, how the request named it, what loaded, its times.
lrec() {
  jq -c '{status, machine, reason: (.reason // null), memoized: (.memo.memoized // null), target,
    lora_on: [.measurements.models[]? | select((.adapters // []) | length > 0) | {slot, lora: [.adapters[] | "\(.model) x\(.strength) \(.layers // "?") layers"]}],
    asked: {lora: (.request.input.lora // null), models: (.request.input.models // null), turbo: (.request.input.turbo // null), upsampling: (.request.input.prompt_upsampling // null)},
    loaded: ((.measurements.execution.on_demand // {}) | keys),
    steps: [(.measurements.attribution.steps // {}) | to_entries[] | "\(.key) \(.value.count)"],
    weights: (.measurements.weights // null), peak_gib: (((.measurements.memory.peak_allocated_bytes // 0) / 1073741824 * 10 | round) / 10),
    ms: {wait: ((.accepted_at_ms // .created_at_ms) - .created_at_ms), start: ((.started_at_ms // .accepted_at_ms // .created_at_ms) - (.accepted_at_ms // .created_at_ms)), run: ((.settled_at_ms // 0) - (.started_at_ms // .accepted_at_ms // .created_at_ms)), execution: (.execution_ms // null)},
    size: [(.result.width // null), (.result.height // null)], seed: (.result.seed // null), runtime: (.measurements.software.runtime // null),
    prompt: ((.result.prompt // "") | .[0:120])}' "$W/records/$1.json" 2> /dev/null | cut -c1-1500
}
# lpic <name>: the picture a run or a box's take left in out/<name>/.
lpic() { find "$W/out/$1" -type f \( -name '*.png' -o -name '*.webp' -o -name '*.jpg' -o -name '*.jpeg' \) 2> /dev/null | sort | head -1; }
# ldiff <label> <with> <without>: the two pictures' difference a channel, and whether both were asked with the same words in the end.
ldiff() {
  local a b; a=$(lpic "$2") b=$(lpic "$3")
  if [ -z "$a" ] || [ -z "$b" ]; then say "DIFF $1: no picture of ${a:+$3}${a:-$2}"; return 1; fi
  python3 -I "$HERE/fixtures/walkthrough/diff.py" "$a" "$b" > "$W/out/diff-$1.json" 2> "$W/out/diff-$1.err"
  local same; same=$(jq -n --slurpfile x "$W/records/$2.json" --slurpfile y "$W/records/$3.json" '($x[0].result.prompt // "x") == ($y[0].result.prompt // "y")' 2> /dev/null)
  say "DIFF $1 ($2 against $3): $(jq -c '{comparable, same_bytes, mean, mean_rgb, max, moved_share, size: .a.size, a: .a.sha256, b: .b.sha256}' "$W/out/diff-$1.json" 2> /dev/null || tail -c 200 "$W/out/diff-$1.err"); the same final prompt: ${same:-?}"
}
# ltake <box>: a box's newest take copied to out/<box>/, and the record of the run that made it.
ltake() {
  PAGE asset "$P" "$1" > "$W/out/$1.asset.json" 2>&1
  local f r; f=$(jq -r '.versions[-1].files // {} | to_entries[] | select(.key == "image") | .value.path // empty' "$W/out/$1.asset.json" 2> /dev/null | tail -1)
  mkdir -p "$W/out/$1" "$W/records"
  [ -z "$f" ] || [[ $f == /* ]] || f=$P/$f
  [ -n "$f" ] && [ -f "$f" ] && cp "$f" "$W/out/$1/" || say "$1: no take's file (${f:-none}); the box says $(jq -c '{failure: .asset.failure, making: .asset.making, versions: .asset.versions}' "$W/out/$1.asset.json" 2> /dev/null | cut -c1-500)"
  r=$(jq -r '.versions[-1].runs[-1] // empty' "$W/out/$1.asset.json" 2> /dev/null)
  # Only the run its own take names: a box with no take has no record here, never another box's.
  : > "$W/records/$1.json"
  [ -z "$r" ] || (cd "$P" && cozy --json run watch "$r" --once --full) > "$W/records/$1.raw" 2>&1 < /dev/null
  [ -z "$r" ] || grep -a '^{' "$W/records/$1.raw" 2> /dev/null | tail -1 > "$W/records/$1.json"
  say "$1's take: run ${r:-none}: $(lrec "$1")"
}
phase_lora() {
  local w=$WORDS bad=0 began=$SECONDS
  DIR=$P c lora-machine-before machine show "$NAME"
  # 1. The command as a person types it, every default (its upsampler rewrites the words): refused or not, beside its twin.
  lrun lora-turbo-default flux.3-klein/image "instructions=$w" seed=7 --lora "$LORA" || bad=$((bad + 1))
  lrun plain-turbo-default flux.3-klein/image "instructions=$w" seed=7
  ldiff turbo-default lora-turbo-default plain-turbo-default
  # Base the other way round, the plain one first: a LoRA that stayed on after the turbo pair's would show here as no difference.
  lrun plain-base-default flux.3-klein/image "instructions=$w" seed=7 turbo=false
  lrun lora-base-default flux.3-klein/image "instructions=$w" seed=7 turbo=false --lora "$LORA" || bad=$((bad + 1))
  ldiff base-default lora-base-default plain-base-default
  # 2. The same pairs with the words as written (no upsampler): only the LoRA differs between twins.
  lrun lora-turbo flux.3-klein/image "instructions=$w" seed=7 prompt_upsampling=false --lora "$LORA" || bad=$((bad + 1))
  lrun plain-turbo flux.3-klein/image "instructions=$w" seed=7 prompt_upsampling=false
  ldiff turbo lora-turbo plain-turbo
  lrun plain-base flux.3-klein/image "instructions=$w" seed=7 turbo=false prompt_upsampling=false
  lrun lora-base flux.3-klein/image "instructions=$w" seed=7 turbo=false prompt_upsampling=false --lora "$LORA" || bad=$((bad + 1))
  ldiff base lora-base plain-base
  DIR=$P c lora-runs-cli run ls
  # 3. The page: the rental preferred in its drawer, each box rendered by Focus's Render. A box the page's hands made no take of is
  # asked as the page asks it, the machine named (page.py), so its take or its refusal is seen either way.
  WALK_BOXES=kdrop,kplain pageflow lora-render
  sed 's/^/    page: /' "$W/page/chrome/lora-render-steps.txt" 2> /dev/null | sed -n '1,/^    page: $/p' | cut -c1-500 >> "$R"
  sed 's/^/    page: /' "$W/page/chrome/lora-render-times.txt" 2> /dev/null | cut -c1-500 >> "$R"
  local b
  for b in kdrop kplain; do
    PAGE asset "$P" "$b" > "$W/out/$b.asset.json" 2>&1
    [ "$(jq -r '.asset.versions // 0' "$W/out/$b.asset.json" 2> /dev/null)" != 0 ] && continue
    say "$b: no take from the page's Render ($(jq -c '.asset.failure' "$W/out/$b.asset.json" 2> /dev/null | cut -c1-400)); asked as the page asks it, on $NAME"
    timeout 1500 python3 -I "$ROOT/e2e/page.py" render "$P" --on "$NAME" "$b" > "$W/out/$b.render.json" 2> "$W/out/$b.render.err"
    say "$b by the page's request: exit $?; $(tail -c 400 "$W/out/$b.render.err" | tr '\n' ' ')"
  done
  ltake kdrop; ltake kplain
  ldiff page kdrop kplain
  [ "$(jq -r '[.measurements.models[]? | select((.adapters // []) | length > 0)] | length' "$W/records/kdrop.json" 2> /dev/null || echo 0)" -ge 1 ] || bad=$((bad + 1))
  DIR=$P c lora-runs-page run ls
  sed -n 1,18p "$W/cli/lora-runs-page.txt" | cut -c1-220 | sed 's/^/    runs: /' >> "$R"
  say "lora: Klein done $((SECONDS - began)) s into the phase; $bad of its 5 LoRA pictures refused, failed or without its LoRA"
  # 4. Fibo's and Krea 2's boxes, a LoRA of their family from the catalog, bare, and its twin: only with Klein clean (a refusal is
  # reported sooner than their downloads take) and inside the time this session was given for them.
  local fam tool l
  for fam in fibo krea; do
    tool=fibo-2/image; [ "$fam" = fibo ] || tool=krea-2/image
    l=$(param "lora_$fam" '')
    [ -n "$l" ] || l=$(jq -rs --arg f "$fam" '[.[] | select((.family // "") | test($f)) | .model] | first // empty' "$W/lora-all.ndjson" 2> /dev/null)
    if [ -z "$l" ]; then say "lora $fam: the catalog lists no LoRA of that family: nothing to run"; continue; fi
    if [ "$bad" != 0 ]; then say "lora $fam: not run ($l): Klein's are reported first"; continue; fi
    if [ $((SECONDS - began)) -gt "$(param others_within_s 1500)" ]; then say "lora $fam: not run ($l): $((SECONDS - began)) s into the phase, past the $(param others_within_s 1500) s given"; continue; fi
    DIR=$P c "lora-show-$fam" lora show "$l"
    say "lora $fam: $l: $(grep -aE '^(kind|adapter) ' "$W/cli/lora-show-$fam.txt" | tr -s ' ' | tr '\n' ';' | cut -c1-300)"
    lrun "lora-$fam" "$tool" "instructions=$w" seed=7 --lora "$l"
    lrun "plain-$fam" "$tool" "instructions=$w" seed=7
    ldiff "$fam" "lora-$fam" "plain-$fam"
  done
  DIR=$P c lora-machine-after machine show "$NAME"; DIR=$P c lora-models-after model ls --on "$NAME"
  grep -aE '^(version|runtime|tensorfs) ' "$W/cli/lora-machine-after.txt" | tr -s ' ' | sed 's/^/    machine: /' >> "$R"
}
# Cold (the coordinator, 11-18, after 44389586877's first run failed `[unhandled_exception]: Building: attention cuda:0 sm89
# sageattention`): does a first FLUX.3 Klein run with prompt upsampling fail that way on any fresh pod, with no LoRA; then in a new
# executor on the same pod (the machine's service started again), without a LoRA and with one; then Fibo's and Krea 2's boxes with a LoRA
# of their family, each beside its twin. Each run's events are kept with this runner's clock (out/<name>.ndjson), its record read for
# its executor, its stages and its attention kernels, and the machine's log around it.
note() { echo "::notice title=walkthrough $1::$(tr '\n' ' ' <<< "$2" | cut -c1-1200)"; say "NOTE $1: $2"; }
# jrun <name> <tool> <args…>: one `cozy --json run` on the rental, each event line led by this runner's clock.
jrun() {
  local n=$1 tool=$2 t0 rc run before after; shift 2
  mkdir -p "$W/out/$n" "$W/records"
  before=$(newest); t0=$(ms)
  ( cd "$P" && timeout "${TO:-2400}" cozy --json run "$tool" "$@" --on "$NAME" -o "$W/out/$n" 2> "$W/out/$n.err" < /dev/null |
      while IFS= read -r line; do printf '%s %s\n' "$EPOCHREALTIME" "$line"; done > "$W/out/$n.ndjson"; exit "${PIPESTATUS[0]}" )
  rc=$?
  printf '%s\t%s\t%s\tcozy --json run %s %s --on %s\n' "$n" "$rc" "$(( $(ms) - t0 ))" "$tool" "$*" "$NAME" >> "$W/cli/times.tsv"
  run=$(sed 's/^[0-9.]* {/{/' "$W/out/$n.ndjson" 2> /dev/null | grep -a '^{' | jq -r 'select(.run.number != null and (.run.kind // "call") == "call") | (.run.name // .run.id)' 2> /dev/null | tail -1)
  if [ -z "$run" ]; then after=$(newest); [ "$after" = "$before" ] || run=$after; fi
  : > "$W/records/$n.json"
  [ -z "$run" ] || (cd "$P" && cozy --json run watch "$run" --once --full) > "$W/records/$n.raw" 2>&1 < /dev/null
  [ -z "$run" ] || grep -a '^{' "$W/records/$n.raw" 2> /dev/null | tail -1 > "$W/records/$n.json"
  [ -z "$run" ] || (cd "$P" && timeout 60 cozy run watch "$run" --once) > "$W/out/$n.watch.txt" 2>&1 < /dev/null
  say "$n: cozy run $tool $* --on $NAME: exit $rc in $(( $(ms) - t0 )) ms, run ${run:-none (no run made)}: $(lrec "$n")"
  say "$n executor: $(jexec "$n")"
  say "$n events: $(jtimeline "$n")"
  [ "$rc" = 0 ] || { say "$n NOT COMPLETED (exit $rc): $(jq -r '"Run \(.name) \(.status) [\(.reason.code // "?")]: \(.reason.message // "?")"' "$W/records/$n.json" 2> /dev/null); stderr: $(tail -c 500 "$W/out/$n.err" | tr '\n' ' ')"
    jq -r '.reason.detail // empty' "$W/records/$n.json" 2> /dev/null | tail -14 | cut -c1-260 | sed 's/^/    /' | tee -a "$R"; }
  return "$rc"
}
# jexec <name>: the run's executor and what it built, from its record: its process, how long before the run it began (0: made for this
# run), each stage's start after the executor's and its length, the attention kernels as the record lists them, the kernels compiled.
jexec() {
  jq -c '(.measurements.execution.executor.started_unix_ms // null) as $e | {
    pid: [.measurements.execution.gpus[]?.pid], executor_started_unix_ms: $e,
    executor_before_run_s: (if $e then (((.started_at_ms // 0) - $e) / 100 | round / 10) else null end),
    executor_legs_ms: (.measurements.execution.executor.legs_ms // null),
    prepared_after_executor_s: (if $e and .measurements.execution.construction.prepared_unix_ms then ((.measurements.execution.construction.prepared_unix_ms - $e) / 100 | round / 10) else null end),
    stages: [(.measurements.attribution.stages // {}) | to_entries | sort_by(.value.started_unix_ms)[] | "\(.key): +\(((.value.started_unix_ms - ($e // .value.started_unix_ms)) / 100 | round) / 10) s after the executor, for \(((.value.ended_unix_ms - .value.started_unix_ms) / 100 | round) / 10) s"],
    settled_after_executor_s: (if $e then (((.settled_at_ms // 0) - $e) / 100 | round / 10) else null end),
    attention: [.measurements.execution.gpus[]? | .attention | "impl \(.impl // ""): " + ([.kernels[]? | "\(.kernel) \(.line // .state)"] | join("; "))],
    kernels: [.measurements.execution.gpus[]? | .kernels[]? | "\(.kernel) \(.source) \(.compile_ms // 0) ms"],
    kernel_notes: [.measurements.observations[]? | select(.name | test("kernel")) | "\(.name)=\(.value) at stage \(.fields.stage // "") step \(.fields.step // "")"]}' "$W/records/$1.json" 2> /dev/null | cut -c1-1800
}
# jtimeline <name>: from its events as they came: the first sight of each stage after the executor's start, and the upsampler's first token.
jtimeline() {
  local e; e=$(jq -r '.measurements.execution.executor.started_unix_ms // .started_at_ms // 0' "$W/records/$1.json" 2> /dev/null)
  sed 's/^\([0-9.]*\) {/{"_clock": \1, /' "$W/out/$1.ndjson" 2> /dev/null | grep -a '^{' | jq -c --argjson e "${e:-0}" 'select((.progress // .run.progress // null) != null) | (.progress // .run.progress) as $p | {clock: ._clock, at: $p.at_ms, stage: $p.stage, done: $p.completed, total: $p.total, unit: ($p.unit // "")}' 2> /dev/null > "$W/out/$1.progress.ndjson"
  jq -rs --argjson e "${e:-0}" '
    def after($x): if $x.at and $x.at > 0 then (($x.at - $e) / 100 | round / 10) else (($x.clock * 1000 - $e) / 100 | round / 10) end;
    (group_by(.stage) | map(min_by(.clock)) | sort_by(.clock) | map("\(.stage) +\(after(.)) s") | join(", ")) as $stages |
    ([.[] | select((.unit == "tokens") and (.done // 0) >= 1)] | min_by(.clock)) as $t |
    "\(length) progress events; first sight of each stage after the executor: \($stages); first token: \(if $t then "\($t.stage) +\(after($t)) s (\($t.done) of \($t.total))" else "no progress event counts tokens" end)"' "$W/out/$1.progress.ndjson" 2> /dev/null | cut -c1-1500
}
# jlogs <name>: the machine's own log around the run: a time window and a line bound, the lines that tell of kernels kept apart.
jlogs() {
  cozy machine logs "$NAME" --since "${2:-4m}" -n 1500 < /dev/null > "$W/out/$1.machine.log" 2>&1
  grep -aiE 'sage|kernel|probe|compil|crash|Building|attention' "$W/out/$1.machine.log" | tail -45 | cut -c1-300 > "$W/out/$1.kernel-lines.log"
  say "$1: the machine's log, $(wc -l < "$W/out/$1.machine.log") lines of the last ${2:-4m}; its lines about kernels ($(wc -l < "$W/out/$1.kernel-lines.log")):"
  sed 's/^/    log: /' "$W/out/$1.kernel-lines.log" >> "$R"
}
# The machine's service started again: its executors end with it, the kernels it compiled stay on its disk.
restarted() {
  local t0 rc i; t0=$(ms)
  (cd "$P" && timeout 300 cozy machine update "$NAME" --gpu-memory-limit off -y) > "$W/cli/restart-$1.txt" 2>&1 < /dev/null; rc=$?
  for i in $(seq 1 120); do [ "$(state "$NAME")" = ready ] && break; sleep 1; done
  say "restart $1: cozy machine update $NAME --gpu-memory-limit off -y: exit $rc, ready $(( $(ms) - t0 )) ms on: $(grep -v '^\s*$' "$W/cli/restart-$1.txt" | tail -2 | tr '\n' ' ' | cut -c1-240)"
}
phase_cold() {
  local w; w=$(param lora_words 'a hand-painted wooden shop sign that reads "COZY BAKERY" in large ornate letters, hanging over a cobblestone street, morning light')
  local a twin picture fam tool l first second one
  DIR=$P c cold-machine-before machine show "$NAME"
  # 1. The first run on the fresh pod: Klein's box, no LoRA, every default (prompt upsampling on).
  jrun cold-plain flux.3-klein/image "instructions=$w" seed=7; a=$?
  jlogs cold-plain 5m
  if [ "$a" = 0 ]; then
    twin=cold-plain
    note "1 cold first run, no LoRA" "PASSED on $NAME: $(jexec cold-plain)"
  else
    note "1 cold first run, no LoRA" "FAILED on $NAME: $(jq -r '"Run \(.name) \(.status) [\(.reason.code // "?")]: \(.reason.message // "?")"' "$W/records/cold-plain.json" 2> /dev/null); $(jexec cold-plain)"
    local again; jrun cold-plain-again flux.3-klein/image "instructions=$w" seed=7; again=$?; twin=cold-plain-again
    note "1 the same run again, right after" "exit $again on $NAME: $(jq -r '"\(.name) \(.status) \(.reason.message // "")"' "$W/records/cold-plain-again.json" 2> /dev/null); $(jexec cold-plain-again)"
  fi
  # A new executor on the same pod, the kernels now compiled once here: without a LoRA (another seed: no answer from memory), then,
  # the service started again, with the LoRA at seed 7: the as-typed pair's LoRA picture.
  restarted plain
  jrun restarted-plain flux.3-klein/image "instructions=$w" seed=9 || { jlogs restarted-plain 2m; jrun restarted-plain-again flux.3-klein/image "instructions=$w" seed=9; }
  restarted lora
  picture=restarted-lora
  jrun restarted-lora flux.3-klein/image "instructions=$w" seed=7 --lora "$LORA" || { jlogs restarted-lora 2m; picture=restarted-lora-again; jrun restarted-lora-again flux.3-klein/image "instructions=$w" seed=7 --lora "$LORA"; }
  note "1 new executors on the same pod" "no LoRA: $(jq -r '"\(.name) \(.status) \(.reason.message // "")"' "$W/records/restarted-plain.json" 2> /dev/null); with the LoRA: $(jq -r '"\(.name) \(.status) \(.reason.message // "")"' "$W/records/restarted-lora.json" 2> /dev/null); executors: $(jq -c '[.measurements.execution.gpus[]?.pid, .measurements.execution.executor.started_unix_ms]' "$W/records/$twin.json" "$W/records/restarted-plain.json" "$W/records/restarted-lora.json" 2> /dev/null | tr '\n' ' ')"
  # 2. The as-typed turbo pair: the LoRA's picture beside its twin, the same seed and words.
  ldiff turbo-default "$picture" "$twin"
  note "2 as-typed turbo pair" "$(grep -a 'DIFF turbo-default' "$R" | tail -1 | cut -c10-700)"
  # 3. Fibo's and Krea 2's boxes with a LoRA of their family, bare, each beside its twin. Fibo's box has an upsampler too: its first
  # run is its package's first on this pod, with the LoRA when Klein's cold run passed, without it when it failed.
  for fam in fibo krea; do
    tool=fibo-2/image; [ "$fam" = fibo ] || tool=krea-2/image
    l=$(param "lora_$fam" '')
    if [ -z "$l" ]; then say "lora $fam: no LoRA named for it: nothing to run"; continue; fi
    DIR=$P c "lora-show-$fam" lora show "$l"
    say "lora $fam: $l: $(grep -aE '^(kind|adapter) ' "$W/cli/lora-show-$fam.txt" | tr -s ' ' | tr '\n' ';' | cut -c1-300)"
    first=lora second=plain; [ "$a" = 0 ] || { first=plain; second=lora; }
    for one in $first $second; do
      if [ "$one" = lora ]; then jrun "lora-$fam" "$tool" "instructions=$w" seed=7 --lora "$l" || { jlogs "lora-$fam" 2m; jrun "lora-$fam-again" "$tool" "instructions=$w" seed=7 --lora "$l"; }
      else jrun "plain-$fam" "$tool" "instructions=$w" seed=7 || { jlogs "plain-$fam" 2m; jrun "plain-$fam-again" "$tool" "instructions=$w" seed=7; }; fi
    done
    first=lora-$fam; [ -n "$(lpic "$first")" ] || first=lora-$fam-again
    second=plain-$fam; [ -n "$(lpic "$second")" ] || second=plain-$fam-again
    ldiff "$fam" "$first" "$second"
    note "3 $fam" "$tool --lora $l: $(jq -r '"\(.name) \(.status) \(.reason.message // "")"' "$W/records/lora-$fam.json" 2> /dev/null), LoRA on $(jq -c '[.measurements.models[]? | select((.adapters // []) | length > 0) | .slot]' "$W/records/$first.json" 2> /dev/null); $(grep -a "DIFF $fam " "$R" | tail -1 | cut -c10-600)"
  done
  DIR=$P c cold-runs run ls
  sed -n 1,20p "$W/cli/cold-runs.txt" | cut -c1-220 | sed 's/^/    runs: /' >> "$R"
  DIR=$P c cold-machine-after machine show "$NAME"
  grep -aE '^(version|runtime|tensorfs) ' "$W/cli/cold-machine-after.txt" | tr -s ' ' | sed 's/^/    machine: /' >> "$R"
}
# The phases in the order asked (`order`): a short session pays for what matters most first. `results` is the page's look at the boxes.
for ph in $(param order 'predict relock updates tools cancel seeds results lora cold lost'); do
  case $ph in
    results) { has tools || has cancel || has seeds; } && [ -n "${NAME:-}" ] && pageflow results ;;
    *) ! has "$ph" || "phase_$ph" ;;
  esac
done
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
