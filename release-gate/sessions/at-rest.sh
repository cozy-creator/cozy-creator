# at-rest (cl-1123): what an encrypted project's drafted box leaves on this computer outside its vault once the project locks. An
# encrypted project's contents must not be readable at rest on this computer once it's locked; its draft's guess is sent to this
# computer's own machine (`local`), which made it ready from a list. This runner's own machine, no rental.
# 0. An encrypted project (`cozy new`'s default) in a folder of a distinctive name, on `local`. Its box of e2e-boxed/plain, put beside
#    the dev home's checkout packages (so it is guessed onto local: an encrypted project's own code stays in its vault and isn't).
# 1. Baseline: another box with words of its own is rendered on local with no page showing the project (nothing guessed), the
#    project locked, and what holds those words, its name or the folder's listed.
# 2. Unlocked again, a draft box with distinctive words; a page shows the project until its guess is ready on local, its words are
#    edited and guessed again, then the project locks with the page still showing it.
# 3. Every file this computer keeps outside the project's folder (the home's .cozy: TensorD's journal, run records, tmp, both logs;
#    .config, .local, /tmp, the runner's temp) and the project's folder itself are searched for the draft's words (both), the box's
#    name and the folder's name; each file holding one is listed, and file names too. The folder's name in a file that held it
#    already at step 0 (where Calcifer keeps a locked project) is by design.
# 4. Control: an unencrypted project's draft box is guessed onto local and gets ready, and Calcifer's log names its box.
# It passes when the draft's words and box name are in no such file, the folder's name in none it wasn't in at step 0, the encrypted
# draft and the plain one both got ready, and the machine lists neither item once their projects are gone from the page.
# With `upgrade`, step 3b: this computer then moves to that dev-build's cozy (`cozy machine update local --tensord`), whose TensorD
# forgets what the build before kept of the drafts, and the search judged is the one after it.
# params:
#   tree     the packages_refs checkout (`-f packages_refs=pk=<packages ref>`, tree pk); e2e-boxed is put in it
#   upgrade  a dev-build run id whose cozy this computer moves to after step 3 (empty: none)
. "$(dirname "$0")/lib.sh"
fail() {
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  cozy machine logs local tensord --since 2h -n 0 > "$OUT/tensord-local.log" 2>&1
  say "FAIL: $*"; RC=1; exit 1
}
TREE=$(param tree '') UPGRADE=$(param upgrade '')
[ -n "$TREE" ] && [ -d "$PACKAGES/$TREE" ] || fail "dispatch with -f packages_refs=<name>=<packages ref> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
V=$(cozy --version | awk '{print $2}')
for tool in jq curl python3 grep find; do command -v "$tool" > /dev/null || fail "no $tool"; done
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cp -r "$ROOT/e2e/cli/project/boxed" "$SOURCE/e2e-boxed" || fail "e2e-boxed not put in the checkout"
bash "$ROOT/e2e/cli/install-vault-driver.sh" fuse3 > "$OUT/fuse.log" 2>&1 || fail "no FUSE: $(tail -c 300 "$OUT/fuse.log")"
SECUREFS=$HOME/.cozy/tools/securefs/$(jq -r .version "$ROOT/crates/project-mount/helper/manifest.json")
if [ ! -x "$SECUREFS/securefs" ]; then
  archive=$(python3 -I "$ROOT/scripts/prepare-securefs.py" --target x86_64-unknown-linux-gnu --out-dir "$RUNNER_TEMP/securefs") || fail "no securefs"
  mkdir -p "$SECUREFS" && chmod 700 "$SECUREFS" &&
    python3 -I -c 'import sys, zipfile; open(sys.argv[2], "wb").write(zipfile.ZipFile(sys.argv[1]).read("securefs"))' "$archive" "$SECUREFS/securefs" &&
    chmod 755 "$SECUREFS/securefs" || fail "securefs not put in $SECUREFS"
fi
# A dev home, its first-party packages from the checkout, and no test diagnostics: Calcifer as a person's.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = false"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"

# The words this session looks for: none is in anything it didn't put there.
TAG=$(head -c 4 /dev/urandom | od -An -tx1 | tr -d ' \n')
FOLDER=sealedwombat$TAG BOX=quokka$TAG DRAFT="heliotrope draft $TAG" EDITED="marzipan edited $TAG"
BASE=baseline$TAG RENDERED="lilac rendered $TAG" PLAIN=plainbox$TAG PLAINWORDS="saffron plain $TAG"
PASS=$RUNNER_TEMP/at-rest-pass
head -c 24 /dev/urandom | base64 | tr -d '\n' > "$PASS" && chmod 600 "$PASS" || fail "no passphrase"
cozy --json new "$OUT/$FOLDER" --passphrase-file "$PASS" --recovery-out "$RUNNER_TEMP/at-rest-recovery" > "$OUT/sealed.json" 2>&1 ||
  fail "cozy new (encrypted): $(tail -c 300 "$OUT/sealed.json")"
SEALED=$(cd "$OUT/$FOLDER" && pwd)
vault() { cozy --json vault --directory "$SEALED" "$@"; }
lock() { vault lock > "$OUT/lock-$1.json" 2>&1 || fail "vault lock ($1): $(tail -c 300 "$OUT/lock-$1.json")"; }
unlock() {
  vault unlock --passphrase-file "$PASS" --idle 0 > "$OUT/unlock-$1.json" 2>&1 || fail "vault unlock ($1): $(tail -c 300 "$OUT/unlock-$1.json")"
  vault status > "$OUT/vault-$1.json" 2>&1 || fail "vault status: $(tail -c 300 "$OUT/vault-$1.json")"
  jq -e '.encrypted == true and .state == "unlocked"' "$OUT/vault-$1.json" > /dev/null || fail "the project isn't encrypted and unlocked: $(cat "$OUT/vault-$1.json")"
}
lock made
unlock made
PLAIN_PROJECT=$RUN_PROJECT RUN_PROJECT=$SEALED
say "cozy $V on this runner's own machine; an encrypted project in $FOLDER ($(grep -o '"id": *"[^"]*"' "$OUT/sealed.json" | head -1)); words: $TAG"

# found <label>: each file outside the vault (and in the project's folder, its ciphertext) holding any of the words, with which;
# and each file named with one. Into found-<label>.txt, one `<file>\t<word>` a line.
ROOTS=("$HOME/.cozy" "$HOME/.config" "$HOME/.local" /tmp "$RUNNER_TEMP" "$SEALED")
[ -n "${XDG_RUNTIME_DIR:-}" ] && [ -d "$XDG_RUNTIME_DIR" ] && ROOTS+=("$XDG_RUNTIME_DIR")
WORDS=("$DRAFT" "$EDITED" "$BOX" "$FOLDER" "$RENDERED" "$BASE" "$PLAINWORDS" "$PLAIN")
found() {
  local out=$OUT/found-$1.txt args=() word file
  for word in "${WORDS[@]}"; do args+=(-e "$word"); done
  : > "$out"
  while IFS= read -r -d '' file; do
    for word in "${WORDS[@]}"; do grep -qaF -e "$word" "$file" 2> /dev/null && printf '%s\t%s\n' "$file" "$word" >> "$out"; done
  done < <(nice -n 19 grep -rlaFZ --exclude-dir=site-packages --exclude-dir=.git "${args[@]}" "${ROOTS[@]}" 2> /dev/null)
  for word in "${WORDS[@]}"; do
    find "${ROOTS[@]}" -name "*$word*" -print 2> /dev/null | sed "s/\$/\tname: $word/" >> "$out"
  done
  sort -u -o "$out" "$out"
  say "$1: $(wc -l < "$out") file and word pairs outside the vault"
  sed "s|$HOME|~|; s|^|  |" "$out" | tee -a "$R"
}
# tables <label>: for each SQLite file found-<label>.txt lists, which of its tables and columns hold which words (a copy read, with
# its WAL).
tables() {
  local db
  for db in $(cut -f1 "$OUT/found-$1.txt" | sort -u); do
    head -c 16 "$db" 2> /dev/null | grep -qa 'SQLite format 3' || continue
    python3 -I - "$db" "$OUT/tables" "${WORDS[@]}" <<'PY' 2>&1 | sed "s|$HOME|~|; s|^|  |" | tee -a "$R"
import os, shutil, sqlite3, sys
db, scratch, words = sys.argv[1], sys.argv[2], sys.argv[3:]
os.makedirs(scratch, exist_ok=True)
copy = os.path.join(scratch, "copy.db")
for suffix in ("", "-wal"):
    if os.path.exists(db + suffix):
        shutil.copyfile(db + suffix, copy + suffix)
    elif os.path.exists(copy + suffix):
        os.remove(copy + suffix)
con = sqlite3.connect(copy)
for (table,) in con.execute("SELECT name FROM sqlite_master WHERE type='table'"):
    for column in [row[1] for row in con.execute(f'PRAGMA table_info("{table}")')]:
        for word in words:
            n = con.execute(f'SELECT count(*) FROM "{table}" WHERE CAST("{column}" AS TEXT) LIKE ?', (f"%{word}%",)).fetchone()[0]
            if n:
                print(f"{db}: {table}.{column} holds '{word}' in {n} rows")
PY
  done
}
# row <box>: the box's guess as the page's board says it.
row() { run_page board | jq -c --arg n "$1" '[.ahead[]? | select((.names // []) | index($n))] | max_by(.run) // empty' 2> /dev/null; }
# ready <box> <s>: its guess is ready on local within <s> s.
ready() {
  local got='' began=$SECONDS
  while [ $((SECONDS - began)) -lt "$2" ]; do
    got=$(row "$1")
    jq -e '.ready == true and .machine == "local"' <<< "${got:-null}" > /dev/null 2>&1 && { say "$1's guess is ready on local in $((SECONDS - began)) s: $got"; return 0; }
    sleep 1
  done
  say "$1's guess isn't ready on local after $2 s: ${got:-no row}"; return 1
}
listed() { cozy --json machine show local 2> /dev/null | jq -c '[(.status.ahead // [])[] | {id, state, stage: (.readiness.stage // "")}]' 2> /dev/null; }
PAGE=''
shown() { python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/show-$1.txt" 2>&1 & PAGE=$!; BG="$BG $PAGE"; sleep 2; }
unshown() { [ -z "$PAGE" ] || kill "$PAGE" 2> /dev/null; PAGE=''; }
MISMATCH=()

found made
(cd "$SEALED" && cozy --json machine use local) > "$OUT/use-sealed.json" 2>&1 || fail "machine use local: $(tail -c 300 "$OUT/use-sealed.json")"
# This computer's machine brought up by an install of e2e-boxed on it, as a person's first `cozy package install` does.
began=$SECONDS
if ! (cd "$PLAIN_PROJECT" && timeout 900 cozy --json package install "$SOURCE/e2e-boxed" --on local) > "$OUT/install-local.json" 2>&1; then
  cozy --json machine show local > "$OUT/local-show.json" 2>&1
  find "$HOME/.cozy" -path '*logs*' -type f -name '*.log*' -newer "$OUT/sealed.json" -exec sh -c 'tail -n 200 "$1" > "$2/machine-$(basename "$1").tail"' _ {} "$OUT" \; 2> /dev/null
  fail "e2e-boxed didn't install on local in $((SECONDS - began)) s: $(tail -c 300 "$OUT/install-local.json"); local: $(jq -c '.status.booting // .state // .' "$OUT/local-show.json" 2> /dev/null | cut -c1-300)"
fi
say "local up, e2e-boxed installed on it, in $((SECONDS - began)) s"
say "step 1: $BASE rendered on local, no page showing the project"
run_page add "$BASE" --tool e2e-boxed/plain --fields "$(jq -nc --arg w "$RENDERED" '{instructions: $w}')" > "$OUT/base-box.json" 2>&1 ||
  fail "$BASE's box: $(tail -c 300 "$OUT/base-box.json")"
timeout 1200 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on local "$BASE" > "$OUT/base-render.txt" 2>&1 ||
  fail "$BASE's render: $(tail -c 400 "$OUT/base-render.txt")"
say "$BASE rendered: $(tail -1 "$OUT/base-render.txt" | cut -c1-300)"
lock baseline
found baseline
tables baseline
unlock draft
say "step 2: $BOX drafted, shown until its guess is ready on local, edited, guessed again, then the project locked as it's shown"
run_page add "$BOX" --tool e2e-boxed/plain --fields "$(jq -nc --arg w "$DRAFT" '{instructions: $w}')" > "$OUT/box.json" 2>&1 ||
  fail "$BOX's box: $(tail -c 300 "$OUT/box.json")"
shown draft
ready "$BOX" 300 || MISMATCH+=("the encrypted project's draft wasn't ready on local")
FIRST=$(row "$BOX")
run_page set "$BOX" --fields "$(jq -nc --arg w "$EDITED" '{instructions: $w}')" > "$OUT/edit.json" 2>&1 || fail "$BOX's edit: $(tail -c 300 "$OUT/edit.json")"
for _ in $(seq 60); do [ "$(row "$BOX" | jq -r .run 2> /dev/null)" != "$(jq -r .run <<< "${FIRST:-null}")" ] && break; sleep 1; done
ready "$BOX" 300 || MISMATCH+=("the encrypted project's edited draft wasn't ready on local")
say "local's list items before the lock: $(listed)"
lock draft
sleep 10
say "local's list items 10 s after the lock: $(listed)"
unshown
sleep 5
AFTER=$(listed)
say "local's list items with the page closed: $AFTER"
[ "$(jq 'length' <<< "${AFTER:-[]}")" = 0 ] || MISMATCH+=("local still lists items once the encrypted project locked and its page closed: $AFTER")
cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
cozy machine logs local tensord --since 2h -n 0 > "$OUT/tensord-local.log" 2>&1 || true
say "step 3: what holds the words now, the project locked"
found locked
tables locked
# The folder's name where step 0 had it is by design; the draft's words and box name anywhere are not.
holding() { awk -F'\t' -v a="$2" -v b="${3:-}" -v c="${4:-}" '{w=$2; sub(/^name: /, "", w)} w == a || w == b || w == c {print $1}' "$OUT/found-$1.txt" | sort -u; }
JUDGED=locked
if [ -n "$UPGRADE" ]; then
  # 3b. This computer moves to `upgrade`'s cozy as a person's does (its TensorD, CLI and Calcifer), and what the build before left
  # is searched for again.
  say "step 3b: this computer's machine moves to the cozy of dev-build $UPGRADE; what the build before left: $(holding locked "$DRAFT" "$EDITED" "$BOX" | sed "s|$HOME|~|" | paste -sd, -)"
  mkdir -p "$RUNNER_TEMP/upgrade" && gh run download "$UPGRADE" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$RUNNER_TEMP/upgrade" > "$OUT/upgrade-download.txt" 2>&1 ||
    fail "dev-build $UPGRADE's cozy: $(tail -c 300 "$OUT/upgrade-download.txt")"
  NEW=$(find "$RUNNER_TEMP/upgrade" -name cozy-linux-x86_64 | head -1)
  [ -n "$NEW" ] && chmod 755 "$NEW" || fail "dev-build $UPGRADE has no cozy-linux-x86_64"
  NEW_V=$("$NEW" --version | awk '{print $2}')
  cozy machine update local --tensord "$NEW" -y > "$OUT/upgrade.txt" 2>&1 || fail "machine update local: $(tail -c 300 "$OUT/upgrade.txt")"
  began=$SECONDS
  until [ "$(cozy --json machine show local 2> /dev/null | jq -r '.status.version // empty')" = "$NEW_V" ]; do
    [ $((SECONDS - began)) -lt 300 ] || fail "local isn't on $NEW_V 300 s after its update: $(cozy --json machine show local 2>&1 | head -c 300)"
    sleep 2
  done
  say "local runs TensorD $NEW_V after $((SECONDS - began)) s; the CLI is $(cozy --version | awk '{print $2}')"
  cozy machine logs local tensord --since 2h -n 0 > "$OUT/tensord-local-upgraded.log" 2>&1 || true
  say "TensorD at its start: $(grep -a "list items' runs forgotten" "$OUT/tensord-local-upgraded.log" | tail -1 | cut -c1-200)"
  grep -aq "list items' runs forgotten" "$OUT/tensord-local-upgraded.log" || MISMATCH+=("the upgraded TensorD didn't say it forgot the list items' runs")
  found upgraded
  tables upgraded
  JUDGED=upgraded
fi
leaks=$(holding "$JUDGED" "$DRAFT" "$EDITED" "$BOX")
folder_new=$(comm -13 <(holding made "$FOLDER") <(holding "$JUDGED" "$FOLDER"))
[ -z "$leaks" ] || MISMATCH+=("$JUDGED: the draft's words or box name are outside the vault: $(cut -f1 <<< "$leaks" | sort -u | sed "s|$HOME|~|" | paste -sd, -)")
[ -z "$folder_new" ] || MISMATCH+=("$JUDGED: the folder's name is in files it wasn't in when the project was made: $(sed "s|$HOME|~|" <<< "$folder_new" | paste -sd, -)")

say "step 4: control, an unencrypted project's draft"
RUN_PROJECT=$PLAIN_PROJECT
(cd "$RUN_PROJECT" && cozy --json machine use local) > "$OUT/use-plain.json" 2>&1 || fail "machine use local: $(tail -c 300 "$OUT/use-plain.json")"
run_page add "$PLAIN" --tool e2e-boxed/plain --fields "$(jq -nc --arg w "$PLAINWORDS" '{instructions: $w}')" > "$OUT/plain-box.json" 2>&1 ||
  fail "$PLAIN's box: $(tail -c 300 "$OUT/plain-box.json")"
shown plain
ready "$PLAIN" 300 || MISMATCH+=("the unencrypted project's draft wasn't ready on local")
unshown
sleep 5
cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
cozy machine logs local tensord --since 2h -n 0 > "$OUT/tensord-local.log" 2>&1 || true
grep -aF "$PLAIN guessed on local" "$OUT/calcifer.log" | tail -1 | sed 's/^/  Calcifer: /' | tee -a "$R"
grep -aqF "$PLAIN guessed on local" "$OUT/calcifer.log" || MISMATCH+=("Calcifer's log doesn't say the unencrypted project's box was guessed")
say "local's list items with no page: $(listed)"
say "TensorD on its list items: $(grep -acE 'cozy:prepare:.*(its request|dropped)' "$OUT/tensord-local.log") lines, e.g. $(grep -aE 'cozy:prepare:.*its request' "$OUT/tensord-local.log" | tail -1 | cut -c1-300)"
[ ${#MISMATCH[@]} = 0 ] || fail "what didn't match: ${MISMATCH[*]}"
say "PASS: nothing of the encrypted project's draft (its words, its box's name, its folder's) is on this computer outside its vault once it locked; both drafts got ready on local"
