# Optional project encryption through the installed CLI, real native mounts and normal
# file/media readers. Source alongside project.sh; no account, model or rental is needed.
projectvault() (
  step "local project encryption: ordinary storage remains available"
  local work secrets P plain view before after code
  work=$(mktemp -d "$OUT/project-vault.XXXXXX")
  secrets=$(mktemp -d "$OUT/vault-credentials.XXXXXX")
  P=$work/encrypted plain=$work/ordinary
  trap 'cozy --json project -p "$P" lock > /dev/null 2>&1 || true; rm -rf -- "$secrets"' EXIT
  printf '%s\n' 'synthetic initial vault passphrase' > "$secrets/initial"
  printf '%s\n' 'synthetic changed vault passphrase' > "$secrets/changed"
  printf '%s\n' 'synthetic rekey vault passphrase' > "$secrets/rekey"
  printf '%s\n' 'deliberately wrong credential' > "$secrets/wrong"
  chmod 600 "$secrets/"*
  cozy --json project new "$plain" > "$work/plain-created.json"
  cozy --json project -p "$plain" vault status > "$work/plain-status.json"
  jq -e '.encrypted == false and .state == "plain"' "$work/plain-status.json" > /dev/null || fail "ordinary project unexpectedly needs encryption"
  cozy --json project -p "$plain" status > "$work/plain-project.json" || fail "ordinary project no longer opens"
  cozy --json project new "$P" > "$work/created.json"
  cozy project -p "$P" export -o "$work/before.yaml"
  mkdir -p "$P/files"
  printf '%s\n' 'SYNTHETIC_PRIVATE_VAULT_CONTENT_1762838400' > "$P/files/private-sentinel.txt"
  base64 -d > "$P/files/image.png" <<< 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=='
  command -v ffmpeg > /dev/null || fail "vault media proof needs ffmpeg"
  command -v ffprobe > /dev/null || fail "vault media proof needs ffprobe"
  ffmpeg -v error -f lavfi -i color=c=blue:s=320x180:r=24:d=2 -f lavfi -i sine=f=440:d=2 -c:v mpeg4 -c:a aac -shortest "$P/files/movie.mp4"
  ffmpeg -v error -f lavfi -i sine=f=440:d=1 "$P/files/audio.wav"
  before=$(sha "$P/files/movie.mp4")

  step "encrypt the same project and read its mounted view"
  cozy --json project -p "$P" encrypt --passphrase-file "$secrets/initial" --recovery-out "$secrets/recovery1" --remove-plaintext --idle 0 > "$work/encrypted.json"
  view=$(jq -r '.vault.view // empty' "$work/encrypted.json")
  [ -n "$view" ] && [ -d "$view" ] || fail "encrypted project has no mounted readable view"
  grep -q SYNTHETIC_PRIVATE_VAULT_CONTENT_1762838400 "$view/files/private-sentinel.txt" || fail "mounted file changed"
  [ "$(sha "$view/files/movie.mp4")" = "$before" ] || fail "mounted video changed"
  ffprobe -v error -show_entries format=duration -of json "$view/files/movie.mp4" > "$work/video.json"
  ffmpeg -v error -ss 1 -i "$view/files/movie.mp4" -frames:v 1 -f null -
  ffprobe -v error -show_entries format=duration -of json "$view/files/audio.wav" > "$work/audio.json"
  cozy project -p "$P" export -o "$work/after-encryption.yaml"
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

  step "locking blocks project reads; wrong credentials do not open it"
  cozy --json project -p "$P" lock > "$work/locked.json"
  jq -e '.vault.state == "locked"' "$work/locked.json" > /dev/null || fail "lock did not complete"
  if cozy --json project -p "$P" status > "$work/locked-read.json" 2> "$work/locked-read.err"; then fail "locked project state was readable"; fi
  if cozy --json project -p "$P" unlock --passphrase-file "$secrets/wrong" > "$work/wrong.json" 2> "$work/wrong.err"; then fail "wrong passphrase unlocked the project"; fi
  if [ -r "$view/files/private-sentinel.txt" ]; then fail "old mount path stayed readable after lock"; fi

  step "saved recovery and passphrase replacement preserve the files"
  cozy --json project -p "$P" unlock --recovery-file "$secrets/recovery1" --idle 0 > "$work/recovered.json"
  cozy --json project -p "$P" vault passphrase --passphrase-file "$secrets/changed" > "$work/passphrase.json"
  cozy --json project -p "$P" vault recovery --recovery-out "$secrets/recovery2" > "$work/replacement-recovery.json"
  cozy --json project -p "$P" lock > /dev/null
  if cozy --json project -p "$P" unlock --passphrase-file "$secrets/initial" > "$work/old-pass.json" 2> "$work/old-pass.err"; then fail "old live passphrase slot remained active"; fi
  if cozy --json project -p "$P" unlock --recovery-file "$secrets/recovery1" > "$work/old-recovery.json" 2> "$work/old-recovery.err"; then fail "old live recovery slot remained active"; fi
  cozy --json project -p "$P" unlock --recovery-file "$secrets/recovery2" --idle 0 > "$work/new-recovery.json"

  step "full local rekey and explicit plaintext export"
  if [ -n "$LOCAL" ]; then
    step "private execution keeps code, inputs, outputs and failure details in protected storage"
    python3 "$E2E/cli/private_execution.py" --project "$P" --view "$(jq -r '.vault.view' "$work/new-recovery.json")" --out "$work/private-execution" --cli "$COZY"
    python3 "$E2E/cli/private_alias.py" --project "$P" --view "$(jq -r '.vault.view' "$work/new-recovery.json")" --passphrase-file "$secrets/changed" --out "$work/private-alias" --cli "$COZY"
  fi
  cozy project -p "$P" export -o "$work/before-rekey.yaml"
  cozy --json project -p "$P" vault rotate --passphrase-file "$secrets/rekey" --recovery-out "$secrets/recovery3" > "$work/rekey.json"
  view=$(jq -r '.vault.view // empty' "$work/rekey.json")
  [ "$(sha "$view/files/movie.mp4")" = "$before" ] || fail "rekey changed original video bytes"
  cozy project -p "$P" export -o "$work/after-rekey.yaml"
  cmp "$work/before-rekey.yaml" "$work/after-rekey.yaml" || fail "rekey changed logical project history"
  cozy --json project -p "$P" vault export --output "$work/exported" > "$work/export.json"
  [ "$(sha "$work/exported/files/movie.mp4")" = "$before" ] || fail "explicit export changed the video"
  cozy --json project -p "$work/exported" status > "$work/export-status.json" || fail "exported ordinary project does not open"

  step "status polling cannot defeat the absolute unlock deadline"
  cozy --json project -p "$P" lock > /dev/null
  cozy --json project -p "$P" unlock --passphrase-file "$secrets/rekey" --idle 0 --max-duration 2 > "$work/timed-unlock.json"
  code=unlocked
  for _ in $(seq 1 80); do
    cozy --json project -p "$P" vault status > "$work/timed-status.json"
    code=$(jq -r .state "$work/timed-status.json")
    [ "$code" != locked ] || break
    sleep 0.25
  done
  [ "$code" = locked ] || fail "polling kept an absolutely expired session unlocked"
  cozy --json project -p "$P" unlock --recovery-file "$secrets/recovery3" --idle 0 > "$work/rekey-recovery.json"
  view=$(jq -r '.vault.view // empty' "$work/rekey-recovery.json")
  [ "$(sha "$view/files/movie.mp4")" = "$before" ] || fail "new-generation recovery lost the video"
  cozy --json project -p "$P" lock > "$work/final-lock.json"
  printf '{"ordinary":true,"mounted_media":true,"lock":true,"recovery":true,"rekey":true,"expiry":true,"synthetic_only":true}\n' > "$work/receipt.json"
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
