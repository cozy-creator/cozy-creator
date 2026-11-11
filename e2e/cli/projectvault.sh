# Optional project encryption through the installed CLI, real native mounts and normal
# file/media readers. Source alongside project.sh; no account, model or rental is needed.
projectvault() (
  step "local project encryption: ordinary storage remains available"
  local work secrets P plain view before after code
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
  cozy --json new "$plain" > "$work/plain-created.json"
  in_project "$plain" --json vault status > "$work/plain-status.json"
  jq -e '.encrypted == false and .state == "plain"' "$work/plain-status.json" > /dev/null || fail "ordinary project unexpectedly needs encryption"
  in_project "$plain" --json --full status > "$work/plain-project.json" || fail "ordinary project no longer opens"
  cozy --json new "$P" > "$work/created.json"
  project_api "$P" /yaml > "$work/before.yaml"
  mkdir -p "$P/files"
  printf '%s\n' 'SYNTHETIC_PRIVATE_VAULT_CONTENT_1762838400' > "$P/files/private-sentinel.txt"
  base64 -d > "$P/files/image.png" <<< 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=='
  command -v ffmpeg > /dev/null || fail "vault media proof needs ffmpeg"
  command -v ffprobe > /dev/null || fail "vault media proof needs ffprobe"
  ffmpeg -v error -f lavfi -i color=c=blue:s=320x180:r=24:d=2 -f lavfi -i sine=f=440:d=2 -c:v mpeg4 -c:a aac -shortest "$P/files/movie.mp4"
  ffmpeg -v error -f lavfi -i sine=f=440:d=1 "$P/files/audio.wav"
  before=$(sha "$P/files/movie.mp4")

  step "recovery output inside the project refuses before encryption"
  if cozy --json vault --directory "$P" encrypt --passphrase-file "$secrets/initial" --recovery-out "$P/unsafe-recovery.txt" --remove-plaintext --idle 0 > "$work/backing-recovery-refused.json" 2> "$work/backing-recovery-refused.err"; then fail "encryption accepted a recovery secret beside its encrypted backing"; fi
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
  [ -n "$view" ] && [ -d "$view" ] || fail "encrypted project has no mounted readable view"
  grep -q SYNTHETIC_PRIVATE_VAULT_CONTENT_1762838400 "$view/files/private-sentinel.txt" || fail "mounted file changed"
  [ "$(sha "$view/files/movie.mp4")" = "$before" ] || fail "mounted video changed"
  ffprobe -v error -show_entries format=duration -of json "$view/files/movie.mp4" > "$work/video.json"
  ffmpeg -v error -ss 1 -i "$view/files/movie.mp4" -frames:v 1 -f null -
  ffprobe -v error -show_entries format=duration -of json "$view/files/audio.wav" > "$work/audio.json"
  project_api "$P" /yaml > "$work/after-encryption.yaml"
  cmp "$work/before.yaml" "$work/after-encryption.yaml" || fail "encryption changed logical project state"
  # The fixture directory contains only encrypted backing storage and a public keybag.
  python3 - "$E2E/cli" "$P" > "$work/plaintext-matches.txt" <<'PY_BACKING_SCAN'
import os
from pathlib import Path
import sys
sys.path.insert(0, sys.argv[1])
from private_execution import contains
markers = [b"SYNTHETIC_PRIVATE_VAULT_CONTENT_1762838400", b"private-sentinel.txt"]
def failed(error):
    raise error
leaks = []
for directory, dirs, files in os.walk(sys.argv[2], followlinks=False, onerror=failed):
    for name in dirs + files:
        path = Path(directory) / name
        assert not path.is_symlink(), "backing scan encountered an unexpected symlink"
    for name in files:
        path = Path(directory) / name
        if any(marker in name.encode() for marker in markers) or contains(path, markers):
            leaks.append(str(path))
print("\n".join(leaks))
assert not leaks, "encrypted backing retains readable private file content or names"
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
  in_project "$P" --json vault lock > /dev/null
  if in_project "$P" --json vault unlock --passphrase-file "$secrets/initial" > "$work/old-pass.json" 2> "$work/old-pass.err"; then fail "old live passphrase slot remained active"; fi
  if in_project "$P" --json vault unlock --recovery-file "$secrets/recovery1" > "$work/old-recovery.json" 2> "$work/old-recovery.err"; then fail "old live recovery slot remained active"; fi
  in_project "$P" --json vault unlock --recovery-file "$secrets/recovery2" --idle 0 > "$work/new-recovery.json"

  step "full local rekey"
  if [ -n "$LOCAL" ]; then
    step "private execution keeps code, inputs, outputs and failure details in protected storage"
    python3 "$E2E/cli/private_execution.py" --project "$P" --view "$(jq -r '.vault.view' "$work/new-recovery.json")" --out "$work/private-execution" --cli "$COZY"
    python3 "$E2E/cli/private_alias.py" --project "$P" --view "$(jq -r '.vault.view' "$work/new-recovery.json")" --passphrase-file "$secrets/changed" --out "$work/private-alias" --cli "$COZY"
    step "a run outliving the idle window holds the vault open; a lock mid-run ends the run, then locks"
    python3 "$E2E/cli/vault_work.py" --project "$P" --passphrase-file "$secrets/changed" --out "$work/vault-work" --cli "$COZY"
  fi
  project_api "$P" /yaml > "$work/before-rekey.yaml"
  cozy --json vault --directory "$P" rotate --passphrase-file "$secrets/rekey" --recovery-out "$secrets/recovery3" > "$work/rekey.json"
  view=$(jq -r '.vault.view // empty' "$work/rekey.json")
  if [ -n "$LOCAL" ]; then
    python3 "$E2E/cli/private_alias.py" --verify-rekey --project "$P" --view "$view" --out "$work/private-rekey" --cli "$COZY"
  fi
  [ "$(sha "$view/files/movie.mp4")" = "$before" ] || fail "rekey changed original video bytes"
  project_api "$P" /yaml > "$work/after-rekey.yaml"
  cmp "$work/before-rekey.yaml" "$work/after-rekey.yaml" || fail "rekey changed logical project history"
  step "status polling cannot defeat the absolute unlock deadline"
  in_project "$P" --json vault lock > /dev/null
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
  project_api "$P" /yaml > "$work/before-decrypt.yaml"
  in_project "$P" --json vault decrypt > "$work/decrypted.json"
  jq -e '.vault.encrypted == false and .vault.state == "plain"' "$work/decrypted.json" > /dev/null || fail "decryption did not complete as ordinary storage"
  [ -f "$P/project.db" ] && [ ! -e "$P/project.vault.keys.json" ] && [ ! -e "$P/project.vault.data" ] || fail "decryption did not replace the encrypted backing"
  [ "$(sha "$P/files/movie.mp4")" = "$before" ] || fail "decryption changed the video"
  in_project "$P" --json --full status > "$work/after-decrypt-status.json"
  [ "$(jq -r '.project.id' "$work/before-decrypt-status.json")" = "$(jq -r '.project.id' "$work/after-decrypt-status.json")" ] || fail "decryption changed the project identity"
  project_api "$P" /yaml > "$work/after-decrypt.yaml"
  cmp "$work/before-decrypt.yaml" "$work/after-decrypt.yaml" || fail "decryption changed project state or history"
  if [ -n "$LOCAL" ]; then
    python3 "$E2E/cli/private_alias.py" --verify-decrypt --project "$P" --view "$P" --out "$work/private-decrypt" --cli "$COZY"
  fi

  step "encrypt the decrypted project again"
  in_project "$P" --json vault encrypt --passphrase-file "$secrets/rekey" --recovery-out "$secrets/recovery4" --remove-plaintext --idle 0 > "$work/reencrypted.json"
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
