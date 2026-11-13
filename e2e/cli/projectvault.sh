# Optional project encryption through the installed CLI, real native mounts and normal
# file/media readers. Source alongside project.sh; no account, model or rental is needed.
projectvault() (
  step "local project encryption: ordinary storage remains available"
  local work secrets P plain view before after code
  # boxes_of <project>: its boxes as the API answers them, the logical state compared across encryption, rekey and decryption.
  boxes_of() {
    local state
    state=$(project_api "$1" '') && jq -S '[.assets[] | {name, tool, fields, approved, pinned}]' <<< "$state" || fail "the project's boxes could not be read"
  }
  work=$(mktemp -d "$OUT/project-vault.XXXXXX")
  secrets=$(mktemp -d "$OUT/vault-credentials.XXXXXX")
  P=$work/encrypted plain=$work/ordinary
  close_vault() {
    local name=$1 code=0
    in_project "$P" --json vault lock > "$work/$name.json" || code=$?
    if [ "$code" = 0 ]; then
      jq -e '.vault.state == "locked"' "$work/$name.json" > /dev/null || fail "lock did not acknowledge locked"
    else
      jq -e '.error.code == "project_busy"' "$work/$name.json" > /dev/null || fail "lock failed without its typed busy response"
      in_project "$P" --json vault status > "$work/$name-status.json"
      jq -e '.state == "locking" or .state == "locked"' "$work/$name-status.json" > /dev/null || fail "busy lock did not revoke the view"
      for _ in $(seq 1 80); do
        [ "$(jq -r .state "$work/$name-status.json")" != locked ] || break
        sleep 0.25
        in_project "$P" --json vault status > "$work/$name-status.json"
      done
      jq -e '.state == "locked"' "$work/$name-status.json" > /dev/null || fail "lock did not finish draining admitted work"
    fi
  }
  trap 'in_project "$P" --json vault lock > /dev/null 2>&1 || true; rm -rf -- "$secrets"' EXIT
  printf '%s\n' 'synthetic initial vault passphrase' > "$secrets/initial"
  printf '%s\n' 'synthetic changed vault passphrase' > "$secrets/changed"
  printf '%s\n' 'synthetic rekey vault passphrase' > "$secrets/rekey"
  printf '%s\n' 'deliberately wrong credential' > "$secrets/wrong"
  chmod 600 "$secrets/"*
  cozy --json new --unencrypted "$plain" > "$work/plain-created.json"
  in_project "$plain" --json vault status > "$work/plain-status.json"
  jq -e '.encrypted == false and .state == "plain"' "$work/plain-status.json" > /dev/null || fail "ordinary project unexpectedly needs encryption"
  in_project "$plain" --json --full status > "$work/plain-project.json" || fail "ordinary project no longer opens"
  step "an ordinary project encrypted by the CLI keeps the absolute expiry it asked for"
  local asked_ms answered_ms
  asked_ms=$(python3 -c 'import time; print(time.time_ns() // 1000000)')
  cozy --json vault --directory "$plain" encrypt --passphrase-file "$secrets/initial" --recovery-out "$secrets/plain-recovery" --idle 0 --max-duration 86400 \
    > "$work/plain-encrypted.json" 2> "$work/plain-encrypted.err" || fail "vault encrypt --max-duration: $(tail -c 300 "$work/plain-encrypted.err")"
  answered_ms=$(python3 -c 'import time; print(time.time_ns() // 1000000)')
  jq -e --argjson b "$asked_ms" --argjson a "$answered_ms" '.vault.state == "unlocked" and .vault.expires_at_ms >= $b + 86400000 and .vault.expires_at_ms <= $a + 86400000' \
    "$work/plain-encrypted.json" > /dev/null || fail "vault encrypt --max-duration 86400: $(jq -c .vault "$work/plain-encrypted.json")"
  in_project "$plain" --json vault lock > "$work/plain-lock.json" || fail "the project the CLI encrypted doesn't lock: $(jq -c '.error | {code, message}' "$work/plain-lock.json" 2> /dev/null)"
  step "a new project encrypted from the start: its passphrase from a file, its recovery secret saved outside it"
  cozy --json new "$work/born" --passphrase-file "$secrets/initial" --recovery-out "$secrets/born-recovery" > "$work/born.json" 2> "$work/born.err" \
    || fail "new --passphrase-file --recovery-out: $(tail -c 300 "$work/born.err")"
  in_project "$work/born" --json vault status > "$work/born-status.json"
  jq -e '.encrypted == true and .state == "unlocked"' "$work/born-status.json" > /dev/null || fail "new --passphrase-file made no unlocked encrypted project: $(cat "$work/born-status.json")"
  [ -s "$secrets/born-recovery" ] || fail "new --recovery-out saved no recovery secret"
  in_project "$work/born" --json vault lock > "$work/born-lock.json" || fail "the new encrypted project doesn't lock: $(jq -c '.error | {code, message}' "$work/born-lock.json" 2> /dev/null)"
  cozy --json new --unencrypted "$P" > "$work/created.json"
  boxes_of "$P" > "$work/before.json"
  mkdir -p "$P/files"
  printf '%s\n' 'SYNTHETIC_PRIVATE_VAULT_CONTENT_1762838400' > "$P/files/private-sentinel.txt"
  base64 -d > "$P/files/image.png" <<< 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=='
  command -v ffmpeg > /dev/null || fail "vault media proof needs ffmpeg"
  command -v ffprobe > /dev/null || fail "vault media proof needs ffprobe"
  ffmpeg -v error -f lavfi -i color=c=blue:s=320x180:r=24:d=2 -f lavfi -i sine=f=440:d=2 -c:v mpeg4 -c:a aac -shortest "$P/files/movie.mp4"
  ffmpeg -v error -f lavfi -i sine=f=440:d=1 "$P/files/audio.wav"
  before=$(sha "$P/files/movie.mp4")

  step "recovery output inside the project refuses before encryption"
  if cozy --json vault --directory "$P" encrypt --passphrase-file "$secrets/initial" --recovery-out "$P/unsafe-recovery.txt" --idle 0 > "$work/backing-recovery-refused.json" 2> "$work/backing-recovery-refused.err"; then fail "encryption accepted a recovery secret beside its encrypted backing"; fi
  jq -e '.error.code == "vault_recovery_location"' "$work/backing-recovery-refused.json" > /dev/null || fail "backing recovery output did not return its location refusal"
  [ ! -e "$P/unsafe-recovery.txt" ] || fail "refused recovery output created a project file"
  in_project "$P" --json vault status > "$work/backing-refusal-status.json"
  jq -e '.encrypted == false and .state == "plain"' "$work/backing-refusal-status.json" > /dev/null || fail "recovery location refusal changed project encryption"
  [ "$(sha "$P/files/movie.mp4")" = "$before" ] || fail "recovery location refusal moved the plaintext source"

  step "encrypt the same project and read its mounted view"
  local encrypt_before_ms encrypt_after_ms
  encrypt_before_ms=$(python3 -c 'import time; print(time.time_ns() // 1000000)')
  python3 "$E2E/cli/vault_stream.py" "$P" "$secrets/initial" "$secrets/recovery1" > "$work/encrypted.json"
  encrypt_after_ms=$(python3 -c 'import time; print(time.time_ns() // 1000000)')
  python3 - "$work/encrypted.json" "$encrypt_before_ms" "$encrypt_after_ms" <<'PY_ENCRYPT_DEADLINE' > "$work/encrypted-deadline.json" || fail "conversion did not retain its requested absolute expiry"
import json,sys
state=json.load(open(sys.argv[1]))['vault']
before,after=map(int,sys.argv[2:])
deadline=state['expires_at_ms']
assert isinstance(deadline,int) and before+86400000 <= deadline <= after+86400000
print(json.dumps({'requested_seconds':86400,'before_ms':before,'after_ms':after,'expires_at_ms':deadline,'passed':True}))
PY_ENCRYPT_DEADLINE
  view=$(jq -r '.vault.view // empty' "$work/encrypted.json")
  jq -e '.stream_proof.default_source_removed' "$work/encrypted.json" > /dev/null || fail "API encryption left a plaintext source"
  [ -n "$view" ] && [ -d "$view" ] || fail "encrypted project has no mounted readable view"
  grep -q SYNTHETIC_PRIVATE_VAULT_CONTENT_1762838400 "$view/files/private-sentinel.txt" || fail "mounted file changed"
  [ "$(sha "$view/files/movie.mp4")" = "$before" ] || fail "mounted video changed"
  ffprobe -v error -show_entries format=duration -of json "$view/files/movie.mp4" > "$work/video.json"
  ffmpeg -v error -ss 1 -i "$view/files/movie.mp4" -frames:v 1 -f null -
  ffprobe -v error -show_entries format=duration -of json "$view/files/audio.wav" > "$work/audio.json"
  boxes_of "$P" > "$work/after-encryption.json"
  cmp "$work/before.json" "$work/after-encryption.json" || fail "encryption changed logical project state"
  # The fixture directory contains only encrypted backing storage and a public keybag. Its open view is mounted inside it
  # (`.vault-runtime/<id>/view`): that mount, and no other, is the readable view, not backing.
  python3 - "$E2E/cli" "$P" "$view" > "$work/plaintext-matches.txt" <<'PY_BACKING_SCAN'
import os
from pathlib import Path
import sys
sys.path.insert(0, sys.argv[1])
from private_execution import contains
markers = [b"SYNTHETIC_PRIVATE_VAULT_CONTENT_1762838400", b"private-sentinel.txt"]
def failed(error):
    raise error
leaks, mounts = [], []
for directory, dirs, files in os.walk(sys.argv[2], followlinks=False, onerror=failed):
    mounted = [name for name in dirs if os.path.ismount(Path(directory) / name)]
    mounts += [Path(directory) / name for name in mounted]
    dirs[:] = [name for name in dirs if name not in mounted]
    for name in dirs + files:
        path = Path(directory) / name
        assert not path.is_symlink(), "backing scan encountered an unexpected symlink"
    for name in files:
        path = Path(directory) / name
        if any(marker in name.encode() for marker in markers) or contains(path, markers):
            leaks.append(str(path))
print("\n".join(leaks))
assert all(mount.resolve() == Path(sys.argv[3]).resolve() for mount in mounts), f"backing holds a mount other than its view: {mounts}"
assert not leaks, "encrypted backing retains readable private file content or names: " + ", ".join(os.path.relpath(leak, sys.argv[2]) for leak in leaks)
PY_BACKING_SCAN

  step "recovery output inside the unlocked view refuses without replacing recovery"
  if in_project "$P" --json vault recovery --recovery-out "$view/unsafe-recovery.txt" > "$work/view-recovery-refused.json" 2> "$work/view-recovery-refused.err"; then fail "recovery replacement accepted its own unlocked view"; fi
  jq -e '.error.code == "vault_recovery_location"' "$work/view-recovery-refused.json" > /dev/null || fail "view recovery output did not return its location refusal"
  [ ! -e "$view/unsafe-recovery.txt" ] || fail "refused recovery output created an unlocked-view file"

  step "locking blocks project reads; wrong credentials do not open it"
  in_project "$P" --json vault lock > "$work/locked.json"
  jq -e '.vault.state == "locked"' "$work/locked.json" > /dev/null || fail "lock did not complete"
  if in_project "$P" --json --full status > "$work/locked-read.json" 2> "$work/locked-read.err"; then fail "locked project state was readable"; fi
  if in_project "$P" --json vault unlock --passphrase-file "$secrets/wrong" > "$work/wrong.json" 2> "$work/wrong.err"; then fail "wrong passphrase unlocked the project"; fi
  if [ -r "$view/files/private-sentinel.txt" ]; then fail "old mount path stayed readable after lock"; fi

  step "saved recovery and passphrase replacement preserve the files"
  # The refused in-view replacement must leave the original recovery credential valid.
  in_project "$P" --json vault unlock --recovery-file "$secrets/recovery1" --idle 0 > "$work/recovered.json"
  in_project "$P" --json vault passphrase --passphrase-file "$secrets/changed" > "$work/passphrase.json"
  in_project "$P" --json vault recovery --recovery-out "$secrets/recovery2" > "$work/replacement-recovery.json"
  in_project "$P" --json vault lock > "$work/replaced-lock.json" || fail "the project doesn't lock after its credentials change: $(jq -c '.error | {code, message}' "$work/replaced-lock.json" 2> /dev/null)"
  if in_project "$P" --json vault unlock --passphrase-file "$secrets/initial" > "$work/old-pass.json" 2> "$work/old-pass.err"; then fail "old live passphrase slot remained active"; fi
  if in_project "$P" --json vault unlock --recovery-file "$secrets/recovery1" > "$work/old-recovery.json" 2> "$work/old-recovery.err"; then fail "old live recovery slot remained active"; fi
  in_project "$P" --json vault unlock --recovery-file "$secrets/recovery2" --idle 0 > "$work/new-recovery.json"

  step "full local rekey"
  if [ -n "$LOCAL" ]; then
    step "private execution keeps code, inputs, outputs and failure details in protected storage"
    python3 "$E2E/cli/private_execution.py" --project "$P" --view "$(jq -r '.vault.view' "$work/new-recovery.json")" --out "$work/private-execution" --cli "$COZY"
    python3 "$E2E/cli/private_alias.py" --project "$P" --view "$(jq -r '.vault.view' "$work/new-recovery.json")" --passphrase-file "$secrets/changed" --out "$work/private-alias" --cli "$COZY"
    step "a run outliving the idle window or the maximum holds the vault open; a lock mid-run ends the run, then locks"
    python3 "$E2E/cli/vault_work.py" --project "$P" --passphrase-file "$secrets/changed" --out "$work/vault-work" --cli "$COZY"
  fi
  boxes_of "$P" > "$work/before-rekey.json"
  cozy --json vault --directory "$P" rotate --passphrase-file "$secrets/rekey" --recovery-out "$secrets/recovery3" > "$work/rekey.json"
  view=$(jq -r '.vault.view // empty' "$work/rekey.json")
  if [ -n "$LOCAL" ]; then
    python3 "$E2E/cli/private_alias.py" --verify-rekey --project "$P" --view "$view" --out "$work/private-rekey" --cli "$COZY"
  fi
  [ "$(sha "$view/files/movie.mp4")" = "$before" ] || fail "rekey changed original video bytes"
  boxes_of "$P" > "$work/after-rekey.json"
  cmp "$work/before-rekey.json" "$work/after-rekey.json" || fail "rekey changed logical project history"
  step "status polling cannot defeat the absolute unlock deadline"
  in_project "$P" --json vault lock > "$work/deadline-lock.json" || fail "the rotated project doesn't lock: $(jq -c '.error | {code, message}' "$work/deadline-lock.json" 2> /dev/null)"
  in_project "$P" --json vault unlock --passphrase-file "$secrets/rekey" --idle 0 --max-duration 2 > "$work/timed-unlock.json"
  code=unlocked
  for _ in $(seq 1 80); do
    in_project "$P" --json vault status > "$work/timed-status.json"
    code=$(jq -r .state "$work/timed-status.json")
    [ "$code" != locked ] || break
    sleep 0.25
  done
  [ "$code" = locked ] || fail "polling kept an absolutely expired session unlocked"
  in_project "$P" --json vault unlock --recovery-file "$secrets/recovery3" --idle 0 > "$work/rekey-recovery.json"
  view=$(jq -r '.vault.view // empty' "$work/rekey-recovery.json")
  [ "$(sha "$view/files/movie.mp4")" = "$before" ] || fail "new-generation recovery lost the video"
  if [ "${RUNNER_OS:-}" = Linux ] && [ -n "$LOCAL" ]; then
    step "private media ranges stay in the mounted project for both binders"
    python3 "$E2E/cli/private_ranges.py" --project "$P" --view "$view" --out "$work/private-ranges.json" --cli "$COZY"
  fi
  close_vault final-lock

  step "decrypt the existing project and retain its history"
  if in_project "$P" --json vault decrypt > "$work/locked-decrypt.json" 2> "$work/locked-decrypt.err"; then fail "a locked project decrypted without authorization"; fi
  jq -e '.error.code == "project_locked"' "$work/locked-decrypt.json" > /dev/null || fail "locked decryption did not return its typed refusal"
  in_project "$P" --json vault unlock --recovery-file "$secrets/recovery3" --idle 0 > "$work/decrypt-unlock.json"
  in_project "$P" --json --full status > "$work/before-decrypt-status.json"
  boxes_of "$P" > "$work/before-decrypt.json"
  in_project "$P" --json vault decrypt > "$work/decrypted.json"
  jq -e '.vault.encrypted == false and .vault.state == "plain"' "$work/decrypted.json" > /dev/null || fail "decryption did not complete as ordinary storage"
  [ -f "$P/project.db" ] && [ ! -e "$P/project.vault.keys.json" ] && [ ! -e "$P/project.vault.data" ] || fail "decryption did not replace the encrypted backing"
  [ "$(sha "$P/files/movie.mp4")" = "$before" ] || fail "decryption changed the video"
  in_project "$P" --json --full status > "$work/after-decrypt-status.json"
  [ "$(jq -r '.project.id' "$work/before-decrypt-status.json")" = "$(jq -r '.project.id' "$work/after-decrypt-status.json")" ] || fail "decryption changed the project identity"
  boxes_of "$P" > "$work/after-decrypt.json"
  cmp "$work/before-decrypt.json" "$work/after-decrypt.json" || fail "decryption changed project state or history"
  if [ -n "$LOCAL" ]; then
    python3 "$E2E/cli/private_alias.py" --verify-decrypt --project "$P" --view "$P" --out "$work/private-decrypt" --cli "$COZY"
  fi

  step "encrypt the decrypted project again"
  in_project "$P" --json vault encrypt --passphrase-file "$secrets/rekey" --recovery-out "$secrets/recovery4" --idle 0 > "$work/reencrypted.json"
  [ -z "$(find "$work" -maxdepth 1 -name '.cozy-project-source-*' -print -quit)" ] || fail "default CLI encryption left a source folder"
  view=$(jq -r '.vault.view // empty' "$work/reencrypted.json")
  [ "$(sha "$view/files/movie.mp4")" = "$before" ] || fail "re-encryption changed the video"
  if [ -n "$LOCAL" ]; then
    python3 "$E2E/cli/private_alias.py" --verify-rekey --project "$P" --view "$view" --out "$work/private-reencrypt" --cli "$COZY"
  fi
  close_vault reencrypted-lock
  trap - EXIT
  rm -rf -- "$secrets"
  printf '{"ordinary":true,"mounted_media":true,"lock":true,"recovery":true,"recovery_output_backing_refused":true,"recovery_output_view_refused":true,"rekey":true,"expiry":true,"decrypt":true,"reencrypt":true,"synthetic_only":true}\n' > "$work/receipt.json"
)

# cl-886: encryption leaves nothing outside its vault. A conversion killed midway is recovered by the next start; Calcifer
# never opens its own `.cozy-*` folders as projects; and no file outside the vault holds the project's plaintext.
vault_leftovers() (
  step "encryption leaves nothing behind: an interrupted conversion recovers; no leftover is listed; no plaintext outside the vault"
  [ "${RUNNER_OS:-Linux}" = Linux ] || { echo "vault leftovers: Linux runners only"; return 0; }
  local work secrets L fake copy id marker before since view runtime
  work=$(mktemp -d "$OUT/vault-leftovers.XXXXXX")
  secrets=$(mktemp -d "$OUT/vault-leftovers-credentials.XXXXXX")
  L=$work/leftovers fake=$work/.cozy-project-source-planted copy=$work/leftovers-copy
  # A Calcifer the interrupt killed starts again before anything else reads this folder: its start detaches the
  # conversion's dead mount and recovers the stage. A mount a failed recovery left (its checks already failed) would
  # break the artifact upload, so it is detached last.
  trap 'cozy --json machine ls > /dev/null 2>&1 || true; in_project "$L" --json vault lock > /dev/null 2>&1 || true; rm -rf -- "$secrets"
    mounts | while IFS= read -r m; do fusermount3 -uz "$m" 2> /dev/null || fusermount -uz "$m" 2> /dev/null || true; done' EXIT
  printf '%s\n' 'synthetic leftovers passphrase' > "$secrets/pass"
  chmod 600 "$secrets/pass"
  # Encrypted views mounted under the fixture, live or dead.
  mounts() { awk '$0 ~ / - fuse\.securefs / {print $5}' /proc/self/mountinfo | grep -F -- "$work/" || true; }
  # What a conversion may leave: in the project, its stage, the original it moves aside, its receipt and its mounts'
  # runtime folders; beside it or in Calcifer's home, where conversions once staged, anything; and any mount.
  left() {
    find "$L" -maxdepth 1 \( -name '.cozy-vault-stage-*' -o -name '.cozy-project-source-*' -o -name '.vault-transition.json' \) -print 2> /dev/null || true
    find "$L/.vault-runtime" -mindepth 1 -maxdepth 1 -type d -print 2> /dev/null || true
    find "$work" -maxdepth 1 \( -name '.cozy-vault-*' -o -name '.cozy-project-source-*' \) -print
    find "$HOME/.cozy/calcifer/vaults" -mindepth 1 -maxdepth 1 \( -name 'stage-*' -o -name 'source-*' -o -name 'decrypt-*' -o -name 'new-*' \) -print 2> /dev/null || true
    mounts
  }
  listed_clean() {
    fixture_projects > "$work/ls-$1.json" || { check "leftovers: project list ($1) failed"; return; }
    J '.projects | all(.path | test("/\\.cozy-") | not)' "$work/ls-$1.json" || check "leftovers: the project list ($1) shows a leftover: $(jq -c '[.projects[].path]' "$work/ls-$1.json")"
  }
  cozy --json new --unencrypted "$L" > "$work/created.json" || fail "leftovers: project new failed"
  id=$(jq -r .id "$work/created.json")

  # Calcifer's own folder is never a project, even one holding a whole project.
  cp -a "$L" "$fake"
  if in_project "$fake" --json status > "$work/own-folder.json" 2> "$work/own-folder.err"; then check "leftovers: a .cozy-* folder opened as a project"; fi
  J '.error.code == "project_own_folder"' "$work/own-folder.json" || check "leftovers: a .cozy-* folder was not refused as Calcifer's own: $(head -c 300 "$work/own-folder.json")"
  listed_clean planted
  rm -rf -- "$fake"
  # A copy of a project takes its own identity, and says so.
  cp -a "$L" "$copy"
  project_api "$copy" '' > "$work/copy.json" || check "leftovers: a copy did not open"
  J --arg i "$id" '.id != $i and ((.warnings // []) | any(.message | contains("is a copy of")))' "$work/copy.json" \
    || check "leftovers: a copy took a new identity without saying so: $(jq -c '{id, warnings}' "$work/copy.json")"
  rm -rf -- "$copy"

  since=$(python3 -c 'import time; print(time.time_ns())')
  marker=$(python3 -c 'import secrets; print("CL886" + secrets.token_hex(16))')
  mkdir -p "$L/files"
  printf 'private %s\n' "$marker" > "$L/files/notes.txt"
  # Large enough to be caught while it copies.
  head -c 536870912 /dev/urandom > "$L/files/large.bin"
  before=$(sha "$L/files/notes.txt")

  python3 "$E2E/cli/vault_interrupt.py" "$L" "$secrets/pass" > "$work/interrupted.json" || fail "leftovers: the conversion could not be interrupted midway"
  for _ in $(seq 1 120); do
    in_project "$L" --json vault status > "$work/recovered.json" 2> "$work/recovered.err" && [ -z "$(left)" ] && break
    sleep 0.5
  done
  [ -z "$(left)" ] || check "leftovers: an interrupted conversion left: $(left | tr '\n' ' ')"
  J '.encrypted == false and .state == "plain"' "$work/recovered.json" || check "leftovers: an interrupted conversion changed the project's storage: $(head -c 300 "$work/recovered.json")"
  [ "$(sha "$L/files/notes.txt")" = "$before" ] || check "leftovers: an interrupted conversion changed the original"
  listed_clean interrupted

  cozy --json vault --directory "$L" encrypt --passphrase-file "$secrets/pass" --recovery-out "$secrets/recovery" --idle 0 > "$work/encrypted.json" \
    || fail "leftovers: encryption after recovery failed: $(head -c 300 "$work/encrypted.json")"
  view=$(jq -r '.vault.view // empty' "$work/encrypted.json")
  [ -n "$view" ] && grep -q "$marker" "$view/files/notes.txt" || check "leftovers: the encrypted project lost its file"
  [ ! -e "$L/files" ] && [ ! -e "$L/project.db" ] || check "leftovers: plaintext stayed in the encrypted project's folder"
  listed_clean encrypted
  in_project "$L" --json vault lock > "$work/locked.json" || check "leftovers: lock failed: $(head -c 300 "$work/locked.json")"
  J '.vault.state == "locked"' "$work/locked.json" || check "leftovers: lock did not finish"
  [ -z "$(left)" ] || check "leftovers: encryption left: $(left | tr '\n' ' ')"
  runtime=$HOME/.cozy/calcifer/vaults/$(printf '%s' "$id" | sha256sum | cut -c1-64)
  [ ! -e "$runtime" ] || check "leftovers: lock left its mount folders: $(find "$runtime" -maxdepth 2 | tr '\n' ' ')"
  listed_clean locked

  # No file outside the vault, written since the marker was planted, holds it: the home (Calcifer's state, logs,
  # inputs, caches and TensorFS stores) and the temp folders.
  MARKER=$marker python3 - "$E2E/cli" "$since" > "$work/plaintext-scan.json" <<'PY_LEFTOVER_SCAN' || check "leftovers: the project's plaintext is outside its vault: $(head -c 600 "$work/plaintext-scan.json")"
import json, os, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from private_execution import contains
since, marker = int(sys.argv[2]), os.environ["MARKER"].encode()
roots = [Path.home(), Path("/tmp"), Path(os.environ.get("TMPDIR", "/tmp")), Path(os.environ.get("RUNNER_TEMP", "/tmp"))]
if os.environ.get("TENSORFS_HOME"):
    roots.append(Path(os.environ["TENSORFS_HOME"]))
roots = sorted({root.resolve() for root in roots})
leaked, seen, checked = [], set(), 0
for root in roots:
    for directory, dirs, files in os.walk(root, followlinks=False):
        for name in files:
            path = Path(directory) / name
            if path in seen or path.is_symlink():
                continue
            seen.add(path)
            try:
                if path.stat().st_mtime_ns < since or not path.is_file():
                    continue
                checked += 1
                if contains(path, [marker]):
                    leaked.append(str(path))
            except OSError:
                continue  # gone meanwhile, or another user's: nothing this run wrote
print(json.dumps({"roots": [str(root) for root in roots], "checked_new_or_modified_files": checked, "leaked_paths": leaked}))
assert checked > 0 and not leaked
PY_LEFTOVER_SCAN
  trap - EXIT
  # The locked project's ciphertext isn't an artifact.
  rm -rf -- "$secrets" "$L"
  jq -c '{interrupted: .interrupted, stages_at_kill: .stages}' "$work/interrupted.json" > "$work/receipt.json"
)

# Same installed candidate, normal browser, no mock HTTP or account service.
vault_web() {
  local chrome pw=$OUT/vault-playwright
  chrome=$(web_chrome)
  [ -n "$chrome" ] && [ -x "$chrome" ] || { check "project vault: native browser is unavailable"; return; }
  mkdir -p "$pw"
  printf '{"private":true}\n' > "$pw/package.json"
  if command -v pnpm > /dev/null; then
    pnpm --dir "$(web_path "$pw")" add --ignore-scripts playwright-core@1.55.0 > "$OUT/vault-playwright.log" 2>&1 || { check "project vault: browser driver install failed"; return; }
  else
    # The CLI runner does not inherit the separate candidate-build job's pnpm setup.
    # Use its existing npm, as web_serves does, without changing the pinned driver.
    npm install -s --ignore-scripts --prefix "$(web_path "$pw")" playwright-core@1.55.0 > "$OUT/vault-playwright.log" 2>&1 || { check "project vault: browser driver install failed"; return; }
  fi
  web_proof vault "$chrome" "$pw" ''
}

# cl-981: an update renames a new cozy over the one Calcifer runs, which goes on running a deleted file, whose `current_exe()` is
# `<path> (deleted)`. An encrypted project then still unlocks: Calcifer starts its filesystem supervisor from the build it runs
# (Linux's /proc/self/exe), never that path (ENOENT) and never the newer file. The new file here is a copy: the same bytes, another
# inode, as every update's rename leaves.
vault_swapped() (
  step "an encrypted project unlocks after Calcifer's binary is replaced on disk (cl-981)"
  [ "${RUNNER_OS:-Linux}" = Linux ] || { echo "vault swapped: Linux runners only"; return 0; }
  local work secrets S pid exe ran sup='' p a b
  work=$(mktemp -d "$OUT/vault-swapped.XXXXXX")
  secrets=$(mktemp -d "$OUT/vault-swapped-credentials.XXXXXX")
  S=$work/swapped
  trap 'in_project "$S" --json vault lock > /dev/null 2>&1 || true; rm -rf -- "$secrets"' EXIT
  printf '%s\n' 'synthetic swapped passphrase' > "$secrets/pass"
  chmod 600 "$secrets/pass"
  cozy --json new "$S" --passphrase-file "$secrets/pass" --recovery-out "$secrets/recovery" > "$work/created.json" 2> "$work/created.err" \
    || fail "vault swapped: new encrypted project: $(tail -c 300 "$work/created.err")"
  in_project "$S" --json vault lock > "$work/locked.json" || fail "vault swapped: lock: $(head -c 300 "$work/locked.json")"
  # Calcifer and its supervisor are nondumpable (#2858): their /proc links read as root only.
  pid=$(jq -r .pid "$HOME/.cozy/calcifer/calcifer.lock.holder") || fail "vault swapped: no Calcifer lock record"
  exe=$(sudo -n readlink "/proc/$pid/exe") || fail "vault swapped: Calcifer ($pid)'s program can't be read"
  [ -f "$exe" ] || fail "vault swapped: Calcifer ($pid) runs $exe, not a file"
  # The update: a new file renamed over Calcifer's while it runs.
  cp -p "$exe" "$exe.cl981" && mv -f "$exe.cl981" "$exe" || fail "vault swapped: $exe could not be replaced"
  ran=$(sudo -n readlink "/proc/$pid/exe") || fail "vault swapped: Calcifer ($pid) is gone after the swap"
  [ "$ran" = "$exe (deleted)" ] || fail "vault swapped: Calcifer runs $ran after the swap, not $exe (deleted)"
  in_project "$S" --json vault unlock --passphrase-file "$secrets/pass" --idle 0 > "$work/unlocked.json" 2> "$work/unlocked.err" \
    || check "vault swapped: unlock after Calcifer's binary was replaced: $(head -c 400 "$work/unlocked.json") $(tail -c 200 "$work/unlocked.err")"
  J '.vault.state == "unlocked"' "$work/unlocked.json" || check "vault swapped: not unlocked: $(head -c 300 "$work/unlocked.json")"
  [ "$(jq -r .pid "$HOME/.cozy/calcifer/calcifer.lock.holder")" = "$pid" ] || check "vault swapped: Calcifer restarted, so the swap went unseen"
  # Its supervisor runs Calcifer's own build: the deleted file, not the new one.
  for p in $(pgrep -f __cozy_vault_provider || true); do
    if sudo -n cat "/proc/$p/cmdline" 2> /dev/null | tr '\0' '\n' | grep -qF "$S/"; then sup=$p; fi
  done
  if [ -z "$sup" ]; then
    if J '.vault.state == "unlocked"' "$work/unlocked.json"; then check "vault swapped: unlocked with no supervisor of $S"; fi
  else
    a=$(sudo -n stat -L -c %d:%i "/proc/$sup/exe") || a="unreadable"
    b=$(sudo -n stat -L -c %d:%i "/proc/$pid/exe") || b="unreadable"
    [ "$a" = "$b" ] && [ "$a" != unreadable ] \
      || check "vault swapped: the supervisor runs $a ($(sudo -n readlink "/proc/$sup/exe" 2>&1)), not Calcifer's build $b"
  fi
  in_project "$S" --json vault lock > "$work/relocked.json" || check "vault swapped: lock after unlock: $(head -c 300 "$work/relocked.json")"
  # Calcifer onto the installed file again, for what follows.
  cozy --json restart > "$work/restart.json" 2> "$work/restart.err" || check "vault swapped: cozy restart: $(tail -c 300 "$work/restart.err")"
  trap - EXIT
  rm -rf -- "$secrets" "$S"
)
