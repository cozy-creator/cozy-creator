# e2e/cli/hardcut.sh: the legacy hard cut (cl-1003) on a home cozy 0.25.0 made and used.

# hard_cut: cozy 0.25.0 runs as another user of this runner in its own default home, with its own Calcifer and local machine
# (old_takes' way). It makes five projects, each with a drawn still, a shot whose latents stay on its local machine (in that
# machine's TensorFS store, nowhere in the project; one object all five share, by its seed) and an uploaded picture (0.25.0
# refuses runs outside a project). The three plain ones also share a shot of their own seed, and the last one has a shot of its own.
# It encrypts and locks the last two, then stops, and ages every use and those bytes past the store's TTL and grace. Before
# this cozy starts on that home, a booting rental with its open order (tied to a project run) and an unsettled run outside any
# project are added to its store, as a laptop updated mid-work has them. Checks:
#   - killed at each step of the cut and of a project's copy at its open (`cut_windows`), the store is the old one whole or the new
#     one whole, and the next start and open finish with every plain project's files as before;
#   - the store is written again: version 9, no legacy table, no old settled run, no ended machine, no machine held, small;
#   - the booting rental and its order, and the unsettled run, are still there; the rental is still listed;
#   - nothing is written into a locked project's folder;
#   - the cut holds every project's files left on local and copies none; cozy doctor says how many projects' files it holds and how
#     to let them go, and no project lost a file; each plain project copies its files in at its open, then its hold goes;
#   - both plain projects have every file, the one only the plain ones share too; a third that shares it, away from its folder
#     at the cut, is held as a locked one, so the store keeps it until that one opens there again and copies it in; once every
#     project has opened, cozy doctor says nothing is held;
#   - the machine keeps the shared latents for the locked ones: each is held in the machine's store (a TensorFS header root beside
#     Calcifer's store), still there after every use and the bytes are 8 days old, TensorD is stopped and a collection ran; the
#     first unlock copies them in and lets go of its hold, the store keeps them for the other; the second does the same, and the
#     store lets them go; at the next start no copy is left to let go of;
#   - the last project's shot of its own seed: once its unlock copied its latents in, the machine's record of that run (the use
#     naming them and its manifest) is gone too, and nothing in the machine's store names the file's digest or its path;
#   - every box of the five (the locked ones once unlocked) lists the files it did and answers each, and its preview, with the
#     same bytes;
#   - the `state.db.pre-*` copies beside the store are gone.
# With CUT_FROM=8 (--shard hardcut-v8) the store is first fenced as a 0.26 RC left one (Paul's laptop's): its project runs in
# legacy tables beside the live ones, cl-1012's machine lines, and the hold an RC read from local's update by hand.
CUT_RELEASE=0.25.0 CUT_RUNTIME=0.46.0 CUT_TENSORFS=0.25.1
CUT_USER=cozy-cut CUT_FROM=${CUT_FROM:-7}
hard_cut() {
step "legacy hard cut (cl-1003): a $CUT_RELEASE home's old runs go, every project file stays (a locked one's from its unlock)"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "hard cut: $CUT_RELEASE runs as another user of a fresh GitHub runner"; return; }
local home P Q R E F pass before facts latents solo twin twinblob record manifests paths pin pinf blob
id "$CUT_USER" > /dev/null 2>&1 || sudo useradd -m "$CUT_USER" || { check "hard cut: no user"; return; }
home=$(getent passwd "$CUT_USER" | cut -d: -f6)
P=$home/plain Q=$home/plain2 R=$home/plain3 E=$home/sealed F=$home/sealed2 pass=$home/pass
mkdir -p "$OUT/cozy-cut"
curl -fsSL --retry 3 -o "$OUT/cozy-cut.tgz" "https://github.com/cozy-creator/cozy-creator/releases/download/v$CUT_RELEASE/cozy-x86_64-unknown-linux-gnu.tar.gz" \
  && tar -xzf "$OUT/cozy-cut.tgz" -C "$OUT/cozy-cut" || { check "hard cut: no cozy $CUT_RELEASE"; return; }
# Its tools as that release had them: its Runtime runs that author API, not this one's.
for tool in draw shaped; do
  curl -fsSL --retry 3 -o "$OUT/cut-$tool.py" "https://raw.githubusercontent.com/cozy-creator/cozy-creator/v$CUT_RELEASE/e2e/cli/project/$tool.py" \
    || { check "hard cut: no $tool.py of $CUT_RELEASE"; return; }
done
sudo install -D -m 755 "$OUT/cozy-cut/cozy-x86_64-unknown-linux-gnu/cozy" /opt/cozy-cut/cozy-old && sudo install -D -m 755 "$COZY" /opt/cozy-cut/cozy-new \
  && sudo install -D -m 644 "$OUT/cut-draw.py" /opt/cozy-cut/draw.py && sudo install -D -m 644 "$OUT/cut-shaped.py" /opt/cozy-cut/shaped.py \
  || { check "hard cut: not installed for its user"; return; }
[ "$(cut_cozy old --version)" = "cozy $CUT_RELEASE" ] || { check "hard cut: it says $(cut_cozy old --version)"; return; }
cut_cozy old --json machine update local --runtime "$CUT_RUNTIME" --tensorfs "$CUT_TENSORFS" -y > "$OUT/cut-update.json" \
  || { check "hard cut: its machine: $(tail -c 400 "$OUT/cut-update.json") $(tail -c 300 "$OUT/cozy-cut.err")"; return; }
printf '%s\n' 'synthetic hard cut passphrase' | sudo -u "$CUT_USER" tee "$pass" > /dev/null && sudo chmod 600 "$pass"
for folder in "$P" "$Q" "$R" "$E" "$F"; do
  cut_cozy old --json new --unencrypted "$folder" > "$OUT/cut-new.json" || { check "hard cut: new $folder: $(tail -c 300 "$OUT/cozy-cut.err")"; return; }
done
cut_fill old "$P" plain twin:9 && cut_fill old "$Q" plain2 twin:9 && cut_fill old "$R" plain3 twin:9 && cut_fill old "$E" sealed && cut_fill old "$F" sealed2 solo:8 || return
# The second and third encrypted after their runs, as Paul's were: what they left on local is only in the machine's store.
for folder in "$E" "$F"; do
  cut_in "$folder" old --json vault encrypt --passphrase-file "$pass" --recovery-out "$folder.recovery" > "$OUT/cut-encrypt.json" \
    || { check "hard cut: encrypt: $(tail -c 300 "$OUT/cut-encrypt.json") $(tail -c 300 "$OUT/cozy-cut.err")"; return; }
  cut_in "$folder" old --json vault lock > "$OUT/cut-lock.json" || { check "hard cut: lock: $(tail -c 300 "$OUT/cut-lock.json")"; return; }
done
sudo pkill -KILL -u "$CUT_USER" || true
while pgrep -u "$CUT_USER" > /dev/null; do sleep 0.2; done
latents=$(jq -r '.shot.latents.sha256 | sub("^sha256:"; "")' "$OUT/cut-sealed-before.json")
# One object the three projects' shots share (the same seed): the store keeps it once.
[ "$(jq -r .shot.latents.sha256 "$OUT/cut-plain-before.json" "$OUT/cut-sealed2-before.json" | sort -u)" = "sha256:$latents" ] \
  || { check "hard cut: the projects' latents differ: $(jq -r .shot.latents.sha256 "$OUT"/cut-*-before.json | tr '\n' ' ')"; return; }
blob=$home/.cache/tensorfs/blobs/${latents:0:2}/${latents:2:2}/$latents
# The last locked project's own latents: no other project's, so its machine's record of that run survives the collection.
solo=$(jq -r '.solo.latents.sha256 | sub("^sha256:"; "")' "$OUT/cut-sealed2-before.json")
[ "$solo" != "$latents" ] || { check "hard cut: the third project's own shot shares the others' latents"; return; }
# What the two plain projects alone share (another seed): no locked project's hold keeps it, so one project's copy letting go of it
# before the other's is made would lose it.
twin=$(jq -r '.twin.latents.sha256 | sub("^sha256:"; "")' "$OUT/cut-plain-before.json")
[ "$(jq -r .twin.latents.sha256 "$OUT/cut-plain2-before.json" "$OUT/cut-plain3-before.json" | sort -u)" = "sha256:$twin" ] && [ "$twin" != "$latents" ] \
  || { check "hard cut: the plain projects' own latents aren't shared: $(jq -r .twin.latents.sha256 "$OUT"/cut-plain*-before.json | tr '\n' ' ')"; return; }
twinblob=$home/.cache/tensorfs/blobs/${twin:0:2}/${twin:2:2}/$twin
# As old as a laptop's: every use and these bytes past the store's TTL and its grace, before the cut.
sudo find "$home/.cache/tensorfs/uses" -type f -exec touch -d '8 days ago' {} + && sudo touch -d '8 days ago' "$blob" "$twinblob" \
  || check "hard cut: uses not aged before the cut"
sudo test -f "$blob" || { check "hard cut: the locked project's latents aren't in its machine's store before the cut"; return; }
cut_sql "SELECT user_version FROM pragma_user_version" > "$OUT/cut-version-before" && [ "$(cat "$OUT/cut-version-before")" = 7 ] \
  || { check "hard cut: $CUT_RELEASE's store is at version $(cat "$OUT/cut-version-before"), not 7"; return; }
cut_live || { check "hard cut: the booting rental and the run going"; return; }
[ "$CUT_FROM" = 7 ] || cut_fence || { check "hard cut: its store not fenced as a 0.26 RC's"; return; }
sudo -u "$CUT_USER" sh -c 'f=$1; cp "$f" "$f.pre-hand-copy" && touch "$f.pre-0.11-20261021T194950Z-wal"' sh "$home/.cozy/calcifer/state.db" \
  || check "hard cut: copies beside the store"
# The third plain project away from the folder the index names (moved; back after the cut).
sudo mv "$R" "$R.away" || { check "hard cut: the third plain project not moved"; return; }
echo "hard cut: before: $(cut_store_facts)"
sudo install -m 644 -o "$(id -u)" "$home/.cozy/calcifer/state.db" "$OUT/cut-store-before.db" || true
before=$(sudo find "$E" "$F" -mindepth 1 -maxdepth 1 -type f -printf '%p %s\n' | sort)
cut_windows "$(cut_store_facts)"

# This cozy on that home, installed in the release's place as an update leaves it (the Calcifer it starts is the installed one).
sudo -u "$CUT_USER" -H sh -c 'printf %s /opt/cozy-cut/cozy-new > "$HOME/.cozy/installed"' || { check "hard cut: this cozy not installed"; return; }
# What it makes once started (an update of its machine) is no old run.
CUT_STARTED_MS=$(($(date +%s%N) / 1000000))
cut_cozy new --json machine ls > "$OUT/cut-machines.json" || {
  check "hard cut: this cozy's first start: $(tail -c 300 "$OUT/cut-machines.json")"
  sudo ls -la "$home/.cozy/calcifer"
  sudo install -m 644 -o "$(id -u)" "$home/.cozy/calcifer/state.db.pre-hand-copy" "$OUT/cut-store-before.db" || true
  sudo find "$home/.cozy/calcifer" "$home/.cozy/services" -maxdepth 2 -name '*.log' -exec sh -c 'echo "== $1"; tail -n 30 "$1"' sh {} \; 2> /dev/null
  return
}
facts=$(cut_store_facts)
echo "hard cut: after: $facts"
J '.version == 9 and .legacy_tables == 0 and .settled_runs == 0 and .ended_machines == 0 and .bytes < 1048576' <<< "$facts" \
  || check "hard cut: its store after the cut: $facts"
[ "$(cut_sql "SELECT count(*) FROM machines WHERE held IS NOT NULL")" = 0 ] \
  || check "hard cut: a machine starts held: $(cut_sql "SELECT name, held FROM machines WHERE held IS NOT NULL")"
J '.machines | any(.name == "cut-rental")' "$OUT/cut-machines.json" \
  || check "hard cut: the booting rental isn't listed: $(jq -c '[.machines[] | {name, state}]' "$OUT/cut-machines.json")"
# Still ends by its name (its Hub may know nothing of it; it isn't a machine this cozy lost).
cut_cozy new --json machine end cut-rental --force -y > "$OUT/cut-rental-end.json" 2>&1 || true
! grep -q 'machine_not_found' "$OUT/cut-rental-end.json" || check "hard cut: the booting rental can't be ended by name: $(tail -c 300 "$OUT/cut-rental-end.json")"
[ "$(cut_sql "SELECT count(*) FROM asks WHERE key = 'cut-order' AND run IS NULL AND machine = 'cut-rental'")" = 1 ] \
  || check "hard cut: the rental's open order: $(cut_sql "SELECT key, state, run, machine FROM asks")"
[ "$(cut_sql "SELECT count(*) FROM runs WHERE id = 'run-cutgoing0000000000000000'")" = 1 ] \
  || check "hard cut: the run going outside any project is gone"
[ -z "$(sudo find "$home/.cozy/calcifer" -maxdepth 1 -name 'state.db.pre-*' -o -maxdepth 1 -name 'state.db.cut*')" ] \
  || check "hard cut: copies left beside the store: $(sudo find "$home/.cozy/calcifer" -maxdepth 1 -name 'state.db.*' -printf '%f ')"
[ "$(sudo find "$E" "$F" -mindepth 1 -maxdepth 1 -type f -printf '%p %s\n' | sort)" = "$before" ] \
  || check "hard cut: a locked project's folder changed: $(sudo find "$E" "$F" -mindepth 1 -maxdepth 1 -type f -printf '%p %s\n' | sort | tr '\n' ' ') (was $(tr '\n' ' ' <<< "$before"))"
if [ "$CUT_FROM" = 8 ]; then
  # A kept machine's lines stay with it, against the new store's machines; an ended one's go with its row (cl-1012).
  [ "$(cut_sql "SELECT group_concat(machine) FROM machine_logs")" = local ] || check "hard cut: machine lines kept: $(cut_sql "SELECT group_concat(machine) FROM machine_logs")"
  cut_sql "SELECT sql FROM sqlite_schema WHERE name = 'machine_logs'" | grep -q 'REFERENCES machines' || check "hard cut: machine_logs no longer names machines"
fi
# Every project held, none copied yet: the cut writes nothing into a project folder, and cozy doctor says what it holds and how to let
# it go; no project's files were lost.
cut_doctor "for 5 projects not opened since the upgrade" "were lost before this upgrade"
for folder in "$P" "$Q"; do sudo test -f "$(cut_kept "$folder")" || check "hard cut: no hold of $folder at the cut"; done
# Each plain project copies its files at its open, then its hold goes.
cut_check "$P" plain
cut_check "$Q" plain2
for folder in "$P" "$Q"; do sudo test ! -e "$(cut_kept "$folder")" || check "hard cut: $folder's hold stays after its open copied its files in"; done
# Both plain projects copied what the plain ones share; the third, away from its folder at the cut, is held as a locked one: its
# two neighbours' copies let go of nothing it shares, and it copies them in when it opens there again.
sudo test -f "$twinblob" || check "hard cut: the plain projects' copies let go of latents a project away from its folder still needs"
sudo mv "$R.away" "$R" || check "hard cut: the third plain project not put back"
# Its TensorD stopped, as at an unlock: only the cut's holds keep what it shares.
cut_tensord_stopped
cut_check "$R" plain3
sudo test ! -e "$twinblob" || check "hard cut: the store still has what the plain projects alone share, all three copied in"

# The latents the three share: held for each locked project past the store's 7 days, through a collection, until its own unlock
# copies them in; the plain project's copy at the cut let go of nothing a locked project still needs, nor the first unlock.
pin=$(cut_kept "$E") pinf=$(cut_kept "$F")
sudo test -f "$pin" && sudo test -f "$pinf" || check "hard cut: nothing holds a locked project's latents"
sudo test -f "$blob" || check "hard cut: the plain project's copy let go of latents the locked projects still need"
# The latents as old as a laptop's: their uses and their bytes 8 days old, past the store's TTL and its grace for new objects.
sudo find "$home/.cache/tensorfs/uses" -type f -exec touch -d '8 days ago' {} + && sudo touch -d '8 days ago' "$blob" \
  && sudo touch -d '1 hour ago' "$home/.cache/tensorfs/uses/.swept" || check "hard cut: uses not aged"
sudo pkill -KILL -u "$CUT_USER" || true
while pgrep -u "$CUT_USER" > /dev/null; do sleep 0.2; done
cut_cozy new --json machine ls > /dev/null || { check "hard cut: this cozy again"; return; }
# A laptop's old latents are past its machine's own 7 days, so its TensorD no longer holds them; here its TensorD is stopped
# instead, and only the cut's holds keep them.
cut_tensord_stopped
cut_admit || check "hard cut: a file into the machine's store, to collect"
[ "$(sudo find "$home/.cache/tensorfs/uses/.swept" -newermt '-5 minutes' | wc -l)" = 1 ] || check "hard cut: no collection ran"
sudo test -f "$blob" || check "hard cut: the locked projects' latents were collected before their unlock"
for use in $(cut_naming uses); do echo "hard cut: a use naming the latents after the collection: ${use##*/}: $(sudo cat "$use" | tr '\n' ' ')"; done
# The machine's record of the run that made the third project's own latents: a use naming them beside its manifest, which names
# the file's path in that run.
record=$(cut_naming uses "$solo" | head -1)
[ -n "$record" ] || check "hard cut: no use names the third project's own latents after the collection"
manifests=$([ -z "$record" ] || sudo grep -vx "$solo" "$record")
for m in $manifests; do echo "hard cut: the run's manifest ${m:0:12}: $(sudo cat "$home/.cache/tensorfs/blobs/${m:0:2}/${m:2:2}/$m" | head -c 400)"; done
paths=$(for m in $manifests; do sudo cat "$home/.cache/tensorfs/blobs/${m:0:2}/${m:2:2}/$m" | jq -r '(.entries // [])[].path, (.files // {} | keys[])' 2> /dev/null; done)
[ -z "$record" ] || [ -n "$paths" ] || check "hard cut: the run's manifest names no file"

cut_unlock "$E" sealed "$pin"
sudo test -f "$blob" || check "hard cut: the first unlock let go of latents the other locked project still needs"
sudo test -f "$pinf" || check "hard cut: the other locked project's hold went with the first unlock"
cut_unlock "$F" sealed2 "$pinf"
sudo test ! -e "$blob" || cut_still_held
# With the last copy in, the machine's record of its run goes too: the use, its manifest, and every mention of the file.
[ -z "$record" ] || sudo test ! -e "$record" || check "hard cut: the use naming the third project's own latents is still there: $(sudo cat "$record" | tr '\n' ' ')"
for m in $manifests; do sudo test ! -e "$home/.cache/tensorfs/blobs/${m:0:2}/${m:2:2}/$m" || check "hard cut: the run's manifest ${m:0:12} is still in the machine's store"; done
[ -z "$(sudo grep -rlF "$solo" "$home/.cache/tensorfs" 2> /dev/null)" ] \
  || check "hard cut: the machine's store still mentions the third project's own latents: $(sudo grep -rlF "$solo" "$home/.cache/tensorfs" | tr '\n' ' ')"
# The path as a manifest names it (`"path":"payload"`): a word alone is also in the store's code.
for path in $paths; do
  [ -z "$(sudo grep -rlF "\"path\":\"$path\"" "$home/.cache/tensorfs" 2> /dev/null)" ] \
    || check "hard cut: the machine's store still names the run's file $path: $(sudo grep -rlF "\"path\":\"$path\"" "$home/.cache/tensorfs" | tr '\n' ' ')"
done
# What a copy couldn't let go of is let go of at the next start: none is left once nothing holds them.
sudo pkill -KILL -u "$CUT_USER" || true
while pgrep -u "$CUT_USER" > /dev/null; do sleep 0.2; done
cut_cozy new --json machine ls > /dev/null || check "hard cut: this cozy after the unlocks"
for _ in $(seq 50); do [ -z "$(sudo find "$home/.cozy/calcifer/kept" -name '*.release')" ] && break; sleep 0.2; done
[ -z "$(sudo find "$home/.cozy/calcifer/kept" -name '*.release')" ] || check "hard cut: copies noted as not let go of: $(sudo find "$home/.cozy/calcifer/kept" -name '*.release' -exec cat {} + | head -c 300)"
# Every project opened since: cozy doctor says nothing is held for one.
cut_doctor "" "not opened since the upgrade|were lost before this upgrade"
for folder in "$E" "$F"; do cut_in "$folder" new --json vault lock > "$OUT/cut-relock.json" || check "hard cut: lock again: $(tail -c 300 "$OUT/cut-relock.json")"; done
sudo pkill -KILL -u "$CUT_USER" || true
}

# cut_windows <facts>: the cut killed at each of its steps, and a project's copy at its first open after it, as a process killed there
# would be: its Calcifer runs under strace, which kills it on entry to one system call on one file (no hook in cozy). After each kill
# the store is the old one whole (as <facts> say) or the new one whole, and the next start and open finish with every plain project's
# files as before and no stopped copy's file left; then the home is put back as it was. A kill can't land between the swap's rename
# and the folder's sync: that window is the file system's.
cut_windows() {
  local old=$1 calc=$home/.cozy/calcifer window path calls when want by state key
  command -v strace > /dev/null || sudo apt-get install -y -qq strace > /dev/null || { check "hard cut: no strace for the kills"; return; }
  # What a start makes (an update of its machine) is no old run.
  CUT_STARTED_MS=$(($(date +%s%N) / 1000000))
  key=$(basename "$(cut_kept "$P")" .cozytensors)
  sudo rm -rf /opt/cozy-cut/home && sudo cp -a "$home" /opt/cozy-cut/home || { check "hard cut: the home not kept for the kills"; return; }
  while IFS='|' read -r window path calls when want by; do
    sudo tee /opt/cozy-cut/cozy-killed > /dev/null <<WRAP
#!/bin/sh
exec strace -f -qq -o /dev/null -P '$path' -e trace=$calls -e inject=$calls:signal=KILL:when=$when /opt/cozy-cut/cozy-new "\$@"
WRAP
    sudo chmod 755 /opt/cozy-cut/cozy-killed
    sudo -u "$CUT_USER" -H sh -c 'printf %s /opt/cozy-cut/cozy-killed > "$HOME/.cozy/installed"'
    if [ "$by" = open ]; then
      # A project's copy is its open's, after the start that cut the store.
      cut_cozy new --json machine ls > "$OUT/cut-killed.json" 2>&1 || check "hard cut: the start before the kill $window: $(tail -c 300 "$OUT/cut-killed.json")"
      cut_api "$P" "" > /dev/null 2>&1 && check "hard cut: not killed $window"
    else
      cut_cozy new --json machine ls > "$OUT/cut-killed.json" 2>&1 && check "hard cut: not killed $window"
    fi
    sudo pkill -KILL -u "$CUT_USER" || true
    while pgrep -u "$CUT_USER" > /dev/null; do sleep 0.2; done
    state=$(cut_whole "$old")
    echo "hard cut: killed $window: the store is $state"
    [ "$state" = "$want whole" ] || check "hard cut: killed $window, the store is $state, not $want whole"
    sudo -u "$CUT_USER" -H sh -c 'printf %s /opt/cozy-cut/cozy-new > "$HOME/.cozy/installed"'
    cut_cozy new --json machine ls > /dev/null || check "hard cut: the start after the kill $window"
    [ "$(cut_whole "$old")" = "new whole" ] || check "hard cut: the start after the kill $window left the store $(cut_whole "$old")"
    cut_check "$P" plain
    cut_check "$Q" plain2
    [ -z "$(sudo find "$P/files" "$Q/files" -name '*.cut-part')" ] || check "hard cut: a stopped copy's file left after the next open, killed $window"
    sudo pkill -KILL -u "$CUT_USER" || true
    while pgrep -u "$CUT_USER" > /dev/null; do sleep 0.2; done
    sudo rsync -a --delete /opt/cozy-cut/home/ "$home/" || { check "hard cut: the home not put back after the kill $window"; return; }
  # <what> | <file> | <system calls> | <which call> | <the store after> | <start, or the project's open>. SQLite's own fchmod as it makes
  # the new file is `fchmod`.
  done <<WINDOWS
holding a project's files|$calc/kept/$key.cozytensors|rename,renameat,renameat2|1|old|start
writing the new store|$calc/state.db.cut|pwrite64,write|1|old|start
the new store synced, before the swap|$calc/state.db.cut|chmod,fchmodat,fchmodat2|1|old|start
the swap|$calc/state.db.cut|rename,renameat,renameat2|1|old|start
deleting the backups|$calc/state.db.pre-hand-copy|unlink,unlinkat|1|new|start
a project's copy at its open, before it takes its place|$P/files/$latents|rename,renameat,renameat2|1|new|open
a project's copies in, before its hold goes|$calc/kept/$key.cozytensors|unlink,unlinkat|1|new|open
WINDOWS
}
# cut_kept <folder>: where the cut holds that project's files left on local (`kept_path`).
cut_kept() {
  echo "$home/.cozy/calcifer/kept/$(printf 'cozy: files a locked project left on local\n%s' "$1" | sha256sum | cut -d' ' -f1).cozytensors"
}
# cut_doctor <said> <never said>: cozy doctor, as the cut's user, says the first (when given) and nothing the second (an ERE) matches.
cut_doctor() {
  cut_cozy new doctor > "$OUT/cut-doctor.txt" 2>&1 || true
  [ -z "$1" ] || grep -qF "$1" "$OUT/cut-doctor.txt" || check "hard cut: cozy doctor doesn't say \"$1\": $(grep -i 'project files' "$OUT/cut-doctor.txt" | head -c 400)"
  ! grep -qE "$2" "$OUT/cut-doctor.txt" || check "hard cut: cozy doctor says $(grep -E "$2" "$OUT/cut-doctor.txt" | head -c 400)"
}
# cut_whole <old facts>: "old whole" when the store is as <old facts> say, "new whole" when it is cut, else what it is.
cut_whole() {
  local facts
  facts=$(cut_store_facts)
  if jq -e --argjson old "$1" '[.version, .legacy_tables, .settled_runs, .ended_machines] == ($old | [.version, .legacy_tables, .settled_runs, .ended_machines])' <<< "$facts" > /dev/null; then
    echo "old whole"
  elif J '.version == 9 and .legacy_tables == 0 and .settled_runs == 0 and .ended_machines == 0' <<< "$facts"; then
    echo "new whole"
  else
    echo "neither: $facts"
  fi
}

# cut_unlock <folder> <tag> <hold>: the locked project unlocked, its TensorD stopped (only the cut's hold keeps its latents); it
# answers every file as before, and its hold is gone.
cut_unlock() {
  cut_tensord_stopped
  cut_in "$1" new --json vault unlock --passphrase-file "$pass" > "$OUT/cut-unlock.json" || { check "hard cut ($2): unlock: $(tail -c 300 "$OUT/cut-unlock.json")"; return; }
  cut_check "$1" "$2"
  sudo test ! -e "$3" || check "hard cut ($2): its hold is still there after its unlock copied its files in"
}

# cut_still_held: the latents every project now holds are still in the machine's store: what still holds them, said.
cut_still_held() {
  check "hard cut: the machine still has the latents its projects now hold ($(cut_holds) live holds name them; TensorD $(pgrep -u "$CUT_USER" -x tensord > /dev/null && echo running || echo stopped))"
  echo "hard cut: TensorFS file roots: $(sudo sh -c 'cat "$1"/files/* 2> /dev/null' sh "$home/.cache/tensorfs" | tr '\n' ' ')"
  echo "hard cut: local refs: $(sudo ls "$home/.cache/tensorfs/local" 2> /dev/null | wc -l); uses naming them: $(cut_naming uses | tr '\n' ' ')"
  sudo tail -n 5 "$home/.cozy/calcifer/health.log"
  echo "hard cut: the latents' bytes: $(sudo stat -c '%s bytes, modified %y' "$blob"); open by: $(sudo fuser -v "$blob" 2>&1 | tr '\n' ' ')"
  sudo tar -C "$home/.cache" --exclude=tensorfs/blobs -czf "$OUT/cut-tensorfs.tgz" tensorfs && sudo chown "$(id -u)" "$OUT/cut-tensorfs.tgz"
}

# cut_tensord_stopped: the cut's user's TensorD ended, so no hold of its own keeps the latents; says how many holds still do.
cut_tensord_stopped() {
  sudo pkill -KILL -u "$CUT_USER" -x tensord || true
  while pgrep -u "$CUT_USER" -x tensord > /dev/null; do sleep 0.2; done
  echo "hard cut: TensorD stopped; live holds naming the locked project's latents: $(cut_holds)"
}
# cut_naming <uses|holds> [digest]: the TensorFS records of that kind that name the shared latents, or `digest` (read as root:
# the folder is its user's alone).
cut_naming() {
  sudo find "$home/.cache/tensorfs/$1" -type f -exec grep -lx "${2:-$latents}" {} + 2> /dev/null
}
# cut_holds: how many TensorFS holds of a live process name the locked project's latents.
cut_holds() {
  local n=0 f
  for f in $(cut_naming holds); do
    sudo flock -n "$f" true 2> /dev/null || n=$((n + 1))
  done
  echo "$n"
}

# cut_live: as a laptop updated mid-work has them, in the stopped 0.25.0 store: a rental booting, its open order tied to one of the
# plain project's runs, and a run outside any project sent to local.
cut_live() {
  sudo -u "$CUT_USER" python3 -I - "$(getent passwd "$CUT_USER" | cut -d: -f6)/.cozy/calcifer/state.db" <<'PY'
import json, sqlite3, sys, time
db = sqlite3.connect(sys.argv[1], isolation_level=None)
now = int(time.time() * 1000)
hub = db.execute("SELECT coalesce((SELECT hub FROM runs WHERE hub <> '' LIMIT 1), 'http://127.0.0.1:1')").fetchone()[0]
db.execute("INSERT INTO machines (name, kind, hub, rental_id, key, state, created_at) VALUES ('cut-rental', 'rental', ?1, 'rental-cut-1', '', 'booting', ?2)", (hub, now))
run = db.execute("SELECT id FROM runs WHERE out_dir LIKE '%/plain/.runs' LIMIT 1").fetchone()[0]
body = json.dumps({"request": {"sku": "cut"}, "quote": None, "stopped": False}).encode()
db.execute("INSERT INTO asks (key, hub, digest, body, machine, state, reason, created_at, kind, run) VALUES ('cut-order', ?1, 'cut', ?2, 'cut-rental', 'failed', 'kept across the cut', ?3, 'rent', ?4)", (hub, body, now, run))
first = (db.execute("SELECT max(number) FROM runs").fetchone()[0] or 0) + 1
db.execute("INSERT INTO runs (id, number, idempotency_key, digest, hub, kind, target, machine, pinned, spec, out_dir, state, created_at) "
           "VALUES ('run-cutgoing0000000000000000', ?1, 'cut-going', 'sha256:00', ?2, 'call', 'cut/going', 'local', 0, X'7B7D', '/tmp/cut-going', 'sent', ?3)", (first, hub, now))
db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
PY
}
# cut_admit: a new file into the cut's user's machine store, through this cozy's Calcifer: the store's upkeep runs as it comes in.
cut_admit() {
  local holder address token file=$OUT/cut-admit.bin
  head -c 4096 /dev/urandom > "$file"
  holder=$(sudo cat "$(getent passwd "$CUT_USER" | cut -d: -f6)/.cozy/calcifer/calcifer.lock.holder") || return
  address=$(jq -r .address <<< "$holder") token=$(jq -r .token <<< "$holder")
  sudo -u "$CUT_USER" curl --fail-with-body -sS -X PUT -H "Authorization: Bearer $token" --data-binary @- "http://$address/v1/files/sha256:$(sha "$file")" < "$file" > /dev/null
}

# cut_cozy <old|new> <cozy args...>: the release or this cozy, as the cut's user, from / (a detached Calcifer keeps its folder).
cut_cozy() {
  local which=$1; shift
  (cd / && sudo -u "$CUT_USER" -H env ${SSL_CERT_FILE:+SSL_CERT_FILE="$SSL_CERT_FILE"} ${COZY_HUB_URL:+COZY_HUB_URL="$COZY_HUB_URL"} \
    "/opt/cozy-cut/cozy-$which" "$@" < /dev/null 2>> "$OUT/cozy-cut.err")
}
# cut_in <folder> <old|new> <cozy args...>: the same, in that folder (a verb of the project there).
cut_in() {
  local folder=$1 which=$2; shift 2
  sudo -u "$CUT_USER" -H env ${SSL_CERT_FILE:+SSL_CERT_FILE="$SSL_CERT_FILE"} ${COZY_HUB_URL:+COZY_HUB_URL="$COZY_HUB_URL"} \
    sh -c 'cd "$1" && shift && exec "$@"' sh "$folder" "/opt/cozy-cut/cozy-$which" "$@" < /dev/null 2>> "$OUT/cozy-cut.err"
}
# cut_api <folder> <suffix> <curl args...>: the serving Calcifer's project API, as its user (a Calcifer answers its own account
# alone) with its lock record's token.
cut_api() {
  local directory=$1 suffix=$2 holder encoded
  shift 2
  holder=$(sudo cat "$(getent passwd "$CUT_USER" | cut -d: -f6)/.cozy/calcifer/calcifer.lock.holder") || return
  encoded=$(python3 -c 'from urllib.parse import quote; import sys; print(quote(sys.argv[1], safe=""))' "$directory")
  sudo -u "$CUT_USER" curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token <<< "$holder")" -H 'cozy-project-storage: 3' "$@" "http://$(jq -r .address <<< "$holder")/v1/projects/$encoded$suffix"
}
# cut_sql <query>: the cut's user's state.db, read without a write (immutable).
cut_sql() {
  sudo python3 - "$(getent passwd "$CUT_USER" | cut -d: -f6)/.cozy/calcifer/state.db" "$1" <<'PY'
import sqlite3, sys
for row in sqlite3.connect(f"file:{sys.argv[1]}?immutable=1", uri=True).execute(sys.argv[2]):
    print(*row)
PY
}
# cut_store_facts: what the cut's user's store holds, as JSON.
cut_store_facts() {
  sudo python3 - "$(getent passwd "$CUT_USER" | cut -d: -f6)/.cozy/calcifer/state.db" "${CUT_STARTED_MS:-99999999999999}" <<'PY'
import json, os, sqlite3, sys
db = sqlite3.connect(f"file:{sys.argv[1]}?immutable=1", uri=True)
started = int(sys.argv[2])
tables = [name for (name,) in db.execute("SELECT name FROM sqlite_schema WHERE type = 'table'")]
one = lambda sql: db.execute(sql).fetchone()[0]
print(json.dumps({"version": one("PRAGMA user_version"), "bytes": os.path.getsize(sys.argv[1]),
    "legacy_tables": sum(1 for name in tables if name.startswith(("legacy_", "cut_"))),
    "settled_runs": one(f"SELECT count(*) FROM runs WHERE state = 'settled' AND created_at < {started}") + (one("SELECT count(*) FROM legacy_project_runs") if "legacy_project_runs" in tables else 0),
    "ended_machines": one("SELECT count(*) FROM machines WHERE state = 'ended'")}))
PY
}
# cut_fill <old|new> <folder> <tag>: its boxes made and rendered on local: a still, a shot whose latents stay on the machine, and an
# uploaded picture. What each box holds is kept in $OUT/cut-<tag>-before.json.
cut_fill() {
  local which=$1 P=$2 tag=$3 extras=${4:-} png=$OUT/cut-$3.png sha i name want=3 extra
  python3 -c 'import os,sys,zlib,struct
w=h=8; raw=b"".join(b"\0"+os.urandom(3)*w for _ in range(h))
c=lambda t,d: struct.pack(">I",len(d))+t+d+struct.pack(">I",zlib.crc32(t+d)&0xffffffff)
open(sys.argv[1],"wb").write(b"\x89PNG\r\n\x1a\n"+c(b"IHDR",struct.pack(">IIBBBBB",w,h,8,2,0,0,0))+c(b"IDAT",zlib.compress(raw))+c(b"IEND",b""))' "$png"
  sha="sha256:$(sha "$png")"
  cut_api "$P" /assets -X POST -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg words "A still of one colour $RANDOM." '{name: "card", exact: true, tool: "/opt/cozy-cut/draw.py/draw", kind: "image", fields: {instructions: $words}}')" > /dev/null \
    && cut_api "$P" /assets -X POST -H 'Content-Type: application/json' \
      -d '{"name": "shot", "exact": true, "tool": "/opt/cozy-cut/shaped.py/shot", "kind": "video", "fields": {"summary": "a grey shot", "beats": [{"description": "one beat"}], "seed": 7}}' > /dev/null \
    || { check "hard cut ($tag): its boxes: $(tail -c 300 "$OUT/cozy-cut.err")"; return 1; }
  # Each extra shot <name>:<seed>: its latents shared with whatever other shot has that seed.
  for extra in $extras; do
    want=$((want + 1))
    cut_api "$P" /assets -X POST -H 'Content-Type: application/json' \
      -d "$(jq -nc --arg name "${extra%%:*}" --argjson seed "${extra##*:}" '{name: $name, exact: true, tool: "/opt/cozy-cut/shaped.py/shot", kind: "video", fields: {summary: "a shot of its own", beats: [{description: "one beat"}], seed: $seed}}')" > /dev/null \
      || { check "hard cut ($tag): its shot $extra: $(tail -c 300 "$OUT/cozy-cut.err")"; return 1; }
  done
  local holder address token
  holder=$(sudo cat "$(getent passwd "$CUT_USER" | cut -d: -f6)/.cozy/calcifer/calcifer.lock.holder")
  address=$(jq -r .address <<< "$holder") token=$(jq -r .token <<< "$holder")
  sudo -u "$CUT_USER" curl --fail-with-body -sS -X PUT -H "Authorization: Bearer $token" --data-binary @- "http://$address/v1/files/$sha" < "$png" > /dev/null \
    && cut_api "$P" /imports -X POST -H 'Content-Type: application/json' -d "$(jq -nc --arg source "$sha" '{source: $source, name: "pic", exact: true}')" > /dev/null \
    || { check "hard cut ($tag): its upload"; return 1; }
  for name in card shot $(for extra in $extras; do printf '%s ' "${extra%%:*}"; done); do
    cut_api "$P" /runs -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: cut-$tag-$name-$RANDOM$RANDOM" -d "{\"names\":[\"$name\"],\"on\":[\"local\"]}" > /dev/null \
      || { check "hard cut ($tag): render $name"; return 1; }
  done
  for i in $(seq 600); do
    cut_api "$P" "" > "$OUT/cut-$tag-state.json" 2> /dev/null || true
    J --argjson want "$want" '[.assets[] | select(.name == "card" or .name == "shot" or .name == "pic" or .name == "solo" or .name == "twin")] | length == $want and all(.versions >= 1 and ((.making // []) | length) == 0)' "$OUT/cut-$tag-state.json" 2> /dev/null && break
    J 'any(.assets[]; .failure != null)' "$OUT/cut-$tag-state.json" 2> /dev/null && { check "hard cut ($tag): a render failed: $(jq -c '[.assets[] | {name, failure}]' "$OUT/cut-$tag-state.json")"; return 1; }
    sleep 1
  done
  cut_files "$P" > "$OUT/cut-$tag-before.json"
  J --argjson want "$want" 'length == $want and (.shot.latents.sha256 | test("^sha256:")) and (.card.image.sha256 | test("^sha256:")) and (.pic.image.sha256 | test("^sha256:"))' "$OUT/cut-$tag-before.json" \
    || { check "hard cut ($tag): its boxes hold $(cat "$OUT/cut-$tag-before.json")"; return 1; }
}
# cut_files <folder>: each box's files now, by output name: its hash and its path in the project, as its Calcifer lists them.
cut_files() {
  cut_api "$1" "" | jq -c '[.assets[] | select(.name == "card" or .name == "shot" or .name == "pic" or .name == "solo" or .name == "twin") | {key: .name, value: (.files // {} | map_values({sha256, path}))}] | from_entries'
}
# cut_check <folder> <tag>: after the cut each box lists the files it did, and the project answers each one, from its own files/,
# with the same bytes, and a picture's or video's preview.
cut_check() {
  local P=$1 tag=$2 sha path code got
  cut_files "$P" > "$OUT/cut-$tag-after.json" || { check "hard cut ($tag): its open after the cut: $(tail -c 300 "$OUT/cozy-cut.err")"; return; }
  cmp -s <(jq -S . "$OUT/cut-$tag-before.json") <(jq -S . "$OUT/cut-$tag-after.json") \
    || check "hard cut ($tag): its boxes list $(cat "$OUT/cut-$tag-after.json"), before $(cat "$OUT/cut-$tag-before.json")"
  while read -r sha path; do
    got=$(cut_api "$P" "/files/${path##*/}" | sha256sum | cut -d' ' -f1)
    [ "sha256:$got" = "$sha" ] || { check "hard cut ($tag): $path answers bytes of $got, not $sha"; continue; }
    case $path in *.png|*.mp4) ;; *) continue ;; esac
    for _ in $(seq 120); do
      code=$(cut_api "$P" "/thumbs/$sha?w=256" -o /dev/null -w '%{http_code}' 2> /dev/null || true)
      [ "$code" = 200 ] && break
      sleep 0.5
    done
    [ "$code" = 200 ] || check "hard cut ($tag): the preview of $path answers $code"
  done < <(jq -r '.[][] | "\(.sha256) \(.path)"' "$OUT/cut-$tag-before.json" | sort -u)
  echo "hard cut ($tag): $(jq -r '[.[][]] | length' "$OUT/cut-$tag-before.json") files answered as before"
}
# cut_fence: the cut's user's 0.25.0 store as a 0.26 RC's fence left it (version 8): each run landing in a project, with its rows,
# moved to legacy tables and indexed by its folder; the project index and unseen marks set aside, a new index beside them.
cut_fence() {
  sudo -u "$CUT_USER" python3 -I - "$(getent passwd "$CUT_USER" | cut -d: -f6)/.cozy/calcifer/state.db" <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1], isolation_level=None)
db.execute("PRAGMA foreign_keys = OFF")
db.execute("BEGIN")
owned = "(out_dir LIKE '%/.runs' OR substr(spec, 1, 19) = CAST('cozy:vault:journal:' AS BLOB))"
db.execute(f"CREATE TEMP TABLE owned AS SELECT id, rtrim(substr(out_dir, 1, length(out_dir) - 5), '/') AS root FROM runs WHERE {owned}")
for table, column in [("runs", "id"), ("outputs", "run"), ("run_logs", "run"), ("asks", "run")]:
    db.execute(f"CREATE TABLE legacy_project_{table} AS SELECT * FROM {table} WHERE {column} IN (SELECT id FROM temp.owned)")
for table, column in [("asks", "run"), ("run_logs", "run"), ("outputs", "run"), ("runs", "id")]:
    db.execute(f"DELETE FROM {table} WHERE {column} IN (SELECT id FROM temp.owned)")
db.execute("CREATE TABLE legacy_run_owners (id TEXT PRIMARY KEY, owner_root TEXT NOT NULL, owner_project TEXT) STRICT")
db.execute("INSERT INTO legacy_run_owners SELECT id, root, NULL FROM temp.owned")
db.execute("ALTER TABLE runs ADD COLUMN owner TEXT NOT NULL DEFAULT ''")
db.execute("ALTER TABLE runs ADD COLUMN custody TEXT NOT NULL DEFAULT 'owned'")
db.execute("ALTER TABLE projects RENAME TO legacy_project_projects")
db.execute("CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT NOT NULL, code TEXT NOT NULL DEFAULT '') STRICT")
db.execute("INSERT INTO projects SELECT id, path, code FROM legacy_project_projects")
db.execute("ALTER TABLE unseen RENAME TO legacy_project_unseen")
# The hold an RC read from local's last update by hand (`held_column`): the cut keeps none.
db.execute("ALTER TABLE machines ADD COLUMN held TEXT")
db.execute("UPDATE machines SET held = (SELECT target FROM runs WHERE kind = 'update' AND machine = 'local' AND status = 'completed' ORDER BY number DESC LIMIT 1) WHERE name = 'local'")
assert db.execute("SELECT held FROM machines WHERE name = 'local'").fetchone()[0], "no hold read from local's update"
# cl-1012's kept machine lines, as an RC with it has them: the live local machine's and an ended rental's.
db.execute("CREATE TABLE IF NOT EXISTS machine_logs (machine TEXT PRIMARY KEY REFERENCES machines (name) ON DELETE CASCADE, text TEXT NOT NULL) STRICT")
db.execute("INSERT OR IGNORE INTO machines (name, kind, key, state, created_at) VALUES ('gone-rental', 'rental', '', 'ended', 1700000000000)")
db.execute("INSERT OR REPLACE INTO machine_logs VALUES ('local', 'the local machine says this'), ('gone-rental', 'an ended rental said this')")
db.execute("PRAGMA user_version = 8")
db.execute("COMMIT")
n = db.execute("SELECT count(*) FROM legacy_project_runs").fetchone()[0]
assert n >= 4, f"only {n} project runs fenced"
print(f"fenced {n} project runs")
PY
}

# hard_cut_scale (--shard hardcut-scale): the cut on a store as big as a laptop's (about 2 GB: old project runs with their specs and
# logs, runs outside any project, ended machines), made from this cozy's own store fenced as a 0.26 RC left it. Its first start
# answers within 10 s. (Kills at each step of the cut are hard_cut's, placed by event.)
hard_cut_scale() {
step "legacy hard cut (cl-1003) on a 2 GB store: its first start answers within 10 s"
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder db=$HOME/.cozy/calcifer/state.db big=$OUT/cut-big began took delay pid state
cozy --json machine ls > /dev/null || { check "cut scale: this cozy"; return; }
pid=$(jq -r .pid < "$lock") && term_calcifer "$pid" || return
python3 - "$db" <<'PY' || { check "cut scale: the big store"; return; }
import os, sqlite3, sys
db = sqlite3.connect(sys.argv[1], isolation_level=None)
db.execute("PRAGMA foreign_keys = OFF")
db.execute("BEGIN")
cols = [r[1] for r in db.execute("PRAGMA table_info(runs)")]
db.execute("CREATE TABLE legacy_project_runs AS SELECT * FROM runs WHERE 0")
db.execute("CREATE TABLE legacy_project_run_logs AS SELECT * FROM run_logs WHERE 0")
db.execute("CREATE TABLE legacy_project_outputs AS SELECT * FROM outputs WHERE 0")
db.execute("CREATE TABLE legacy_run_owners (id TEXT PRIMARY KEY, owner_root TEXT NOT NULL, owner_project TEXT) STRICT")
spec = os.urandom(96 * 1024)
def run(table, i, out):
    db.execute(f"INSERT INTO {table} (id, number, idempotency_key, digest, hub, kind, target, pinned, spec, out_dir, state, status, created_at, settled_at) "
               "VALUES (?, ?, ?, 'sha256:00', '', 'call', 'scale/old', 0, ?, ?, 'settled', 'completed', 1700000000000, 1700000000000)",
               (f"run-scale{i:019d}", 1000000 + i, f"scale-{i}", spec, out))
for i in range(15000):
    run("legacy_project_runs", i, f"/gone/project-{i % 700}/.runs")
    db.execute("INSERT INTO legacy_run_owners VALUES (?, ?, NULL)", (f"run-scale{i:019d}", f"/gone/project-{i % 700}"))
    for seq in range(10):
        db.execute("INSERT INTO legacy_project_run_logs (run, seq, at_ms, level, text) VALUES (?, ?, 0, 'info', ?)", (f"run-scale{i:019d}", seq, "x" * 1500))
for i in range(15000, 20000):
    run("runs", i, "/tmp/outside")
for i in range(3000):
    db.execute("INSERT INTO machines (name, kind, key, state, created_at) VALUES (?, 'rental', '', 'ended', 1700000000000)", (f"scale-ended-{i}",))
if "custody" not in cols:
    db.execute("ALTER TABLE runs ADD COLUMN custody TEXT NOT NULL DEFAULT 'owned'")
db.execute("PRAGMA user_version = 8")
db.execute("COMMIT")
db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
print(f"{os.path.getsize(sys.argv[1]) >> 20} MiB")
PY
rm -f "$db-wal" "$db-shm" && cp "$db" "$big" || { check "cut scale: its copy"; return; }
cp "$big" "$db" && rm -f "$db-wal" "$db-shm" "$db.cut"
began=$(date +%s%N)
cozy --json machine ls > "$OUT/cut-scale-ls.json" 2> "$OUT/cut-scale-ls.err" || check "cut scale: its first start: $(tail -c 300 "$OUT/cut-scale-ls.err")"
took=$(( ($(date +%s%N) - began) / 1000000 ))
echo "cut scale: the first start on the $(( $(stat -c %s "$big") >> 20 )) MiB store answered in ${took} ms; the store is $(( $(stat -c %s "$db") >> 10 )) KiB"
[ "$took" -lt 10000 ] || check "cut scale: its first start took ${took} ms"
[ "$(sqlite3 "file:$db?mode=ro" 'PRAGMA user_version' 2> /dev/null || python3 -c 'import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute("PRAGMA user_version").fetchone()[0])' "$db")" = 9 ] || check "cut scale: not cut after the last start"
rm -f "$big"
}

