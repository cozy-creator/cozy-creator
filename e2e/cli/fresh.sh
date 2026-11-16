# e2e/cli/fresh.sh: the `fresh` shard (cl-1030): what cozy needs is on the computer once its installer is done. The last released
# installer, with this tree's scripts/patch-installers-fix.sh, installs the candidate cozy served as the release's archive
# (COZY_DOWNLOAD_URL), and runs `cozy doctor --fix` as a release's installer will:
#   Linux: a fresh Ubuntu 24.04 (no FUSE): a person with sudo answers its one question, then `cozy new` makes an encrypted project;
#          and someone with no sudo and no terminal gets the command, and an install that completes.
#   Windows: WinFsp installed by it from its pinned MSI with every feature (the runner has none), and ffmpeg and uv fetched.
#   macOS: ffmpeg fetched by it, then the first thumbnail fetches nothing.
fresh_here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# fresh_release <dir>: the last release's installers with this tree's fix, and the candidate cozy as this computer's archive, served at
# COZY_DOWNLOAD_URL. The shell installer holds its archive to the digest it carries, as dist writes it.
fresh_release() {
  local dir=$1 target archive staging sum
  mkdir -p "$dir"
  curl -LsSf --retry 3 --retry-all-errors https://tensorhub.com/install.sh -o "$dir/cozy-installer.sh" || return
  curl -LsSf --retry 3 --retry-all-errors https://tensorhub.com/install.ps1 -o "$dir/cozy-installer.ps1" || return
  bash "$fresh_here/../../scripts/patch-installers-fix.sh" "$dir" || return
  case ${RUNNER_OS:-Linux} in
    Windows)
      python - "$COZY" "$dir/cozy-x86_64-pc-windows-msvc.zip" <<'PY' || return
import sys, zipfile
with zipfile.ZipFile(sys.argv[2], "w", zipfile.ZIP_DEFLATED) as out:
    out.write(sys.argv[1], "cozy.exe")
PY
      ;;
    *)
      target=x86_64-unknown-linux-gnu
      [ "${RUNNER_OS:-Linux}" != macOS ] || target=aarch64-apple-darwin
      archive=cozy-$target.tar.gz staging=$(mktemp -d)
      mkdir "$staging/cozy-$target" && cp "$COZY" "$staging/cozy-$target/cozy" && tar czf "$dir/$archive" -C "$staging" "cozy-$target" || return
      rm -rf "$staging"
      sum=$(python3 -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$dir/$archive")
      ARCHIVE=$archive SUM=$sum perl -0pi -e 's/("\Q$ENV{ARCHIVE}\E"\)\n(?:[^\n]*\n)*?\s*_checksum_value=")[0-9a-f]+/$1$ENV{SUM}/' "$dir/cozy-installer.sh"
      grep -q "_checksum_value=\"$sum\"" "$dir/cozy-installer.sh" || { echo "the installer carries no digest for $archive" >&2; return 1; }
      ;;
  esac
  local port=$((20000 + RANDOM % 20000)) py=python3
  command -v python3 > /dev/null || py=python
  (cd "$dir" && exec "$py" -m http.server "$port" --bind 127.0.0.1 > "$OUT/fresh-server.log" 2>&1) &
  FRESH_URL=http://127.0.0.1:$port
  for _ in $(seq 30); do curl -fsS -o /dev/null "$FRESH_URL/cozy-installer.sh" && return 0; sleep 1; done
  echo "the release isn't served at $FRESH_URL" >&2
  return 1
}

fresh_install() {
  step "fresh: the release's installer with doctor --fix"
  fresh_release "$OUT/fresh-release" || { check "fresh: the release couldn't be made: $(tail -c 300 "$OUT/fresh-server.log" 2> /dev/null)"; return 0; }
  case ${RUNNER_OS:-Linux} in
    Linux) fresh_ubuntu; fresh_no_terminal ;;
    Windows) fresh_windows ;;
    macOS) fresh_macos ;;
  esac
}

# A fresh Ubuntu 24.04, its FUSE device lent: a person with sudo pastes the install line in a terminal and answers its one question.
fresh_ubuntu() {
  step "fresh: Ubuntu with no FUSE, a person with sudo, then an encrypted project"
  sudo modprobe fuse 2> /dev/null || true
  [ -c /dev/fuse ] || { check "fresh: this runner has no /dev/fuse to lend"; return 0; }
  docker run --rm --network host --device /dev/fuse --cap-add SYS_ADMIN --security-opt apparmor=unconfined -e URL="$FRESH_URL" \
    ubuntu:24.04 bash -euo pipefail -c '
      # What the person has before pasting the line: curl, sudo, and no FUSE.
      apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq curl ca-certificates sudo bsdutils > /dev/null
      ! command -v fusermount3 > /dev/null || { echo "FRESH-FAIL: fusermount3 before the install"; exit 1; }
      useradd -m alice && echo "alice ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/alice
      # Its terminal (script), and the person answering its one question.
      printf "y\n" | su alice -c "script -qec \"curl -LsSf $URL/cozy-installer.sh | COZY_DOWNLOAD_URL=$URL sh\" /dev/null" > /tmp/install.txt 2>&1 || true
      cat /tmp/install.txt
      grep -q "Install them now with sudo" /tmp/install.txt || { echo "FRESH-FAIL: no question asked"; exit 1; }
      command -v fusermount3 > /dev/null || { echo "FRESH-FAIL: no fusermount3 after the install"; exit 1; }
      su alice -c "cd ~ && printf \"a long enough passphrase\" > pass && ~/.cozy/bin/cozy new vaulted --passphrase-file pass --recovery-out recovery && cd vaulted && ~/.cozy/bin/cozy --json vault status" > /tmp/vault.json 2>&1 \
        || { cat /tmp/vault.json; echo "FRESH-FAIL: cozy new made no encrypted project"; exit 1; }
      cat /tmp/vault.json
      grep -q "\"encrypted\": *true" /tmp/vault.json || { echo "FRESH-FAIL: the project is not encrypted"; exit 1; }
    ' > "$OUT/fresh-ubuntu.txt" 2>&1 || true
  ! grep -q FRESH-FAIL "$OUT/fresh-ubuntu.txt" && grep -q '"encrypted": *true' "$OUT/fresh-ubuntu.txt" \
    || check "fresh: Ubuntu: $(grep FRESH-FAIL "$OUT/fresh-ubuntu.txt" | head -1 || tail -c 400 "$OUT/fresh-ubuntu.txt")"
}

# Someone with no sudo and no terminal: the install completes, and says the command that installs the rest.
fresh_no_terminal() {
  step "fresh: Ubuntu, no sudo and no terminal: the install completes and says the command"
  docker run --rm --network host -e URL="$FRESH_URL" ubuntu:24.04 bash -euo pipefail -c '
      apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq curl ca-certificates > /dev/null
      useradd -m bob
      su bob -c "curl -LsSf $URL/cozy-installer.sh | COZY_DOWNLOAD_URL=$URL sh" < /dev/null > /tmp/install.txt 2>&1 || { cat /tmp/install.txt; echo "FRESH-FAIL: the install failed"; exit 1; }
      cat /tmp/install.txt
      su bob -c "~/.cozy/bin/cozy --version" || { echo "FRESH-FAIL: no cozy after the install"; exit 1; }
      grep -q "apt-get install -y .*fuse3" /tmp/install.txt || { echo "FRESH-FAIL: the command was not said"; exit 1; }
    ' > "$OUT/fresh-no-terminal.txt" 2>&1 || true
  ! grep -q FRESH-FAIL "$OUT/fresh-no-terminal.txt" && grep -q "apt-get install -y .*fuse3" "$OUT/fresh-no-terminal.txt" \
    || check "fresh: no terminal: $(grep FRESH-FAIL "$OUT/fresh-no-terminal.txt" | head -1 || tail -c 400 "$OUT/fresh-no-terminal.txt")"
}

# Windows with no WinFsp: the installer, as an administrator runs it here, installs it from its pinned MSI with every feature, and
# fetches ffmpeg and uv; doctor then finds each. An encrypted project through Calcifer on Windows is its own fix's cell (tracker
# cl-1049: provider mount failed with WinFsp installed, run 43787077181).
fresh_windows() {
  step "fresh: Windows with no WinFsp: the installer installs it and fetches ffmpeg and uv"
  local dll="/c/Program Files (x86)/WinFsp/bin/winfsp-x64.dll" row
  [ "${RUNNER_ARCH:-}" != ARM64 ] || dll="/c/Program Files (x86)/WinFsp/bin/winfsp-a64.dll"
  [ ! -f "$dll" ] || check "fresh: Windows: WinFsp was here before the install, so the install proves nothing"
  powershell -NoProfile -ExecutionPolicy Bypass -c "\$env:COZY_DOWNLOAD_URL = '$FRESH_URL'; & '$(cygpath -w "$OUT/fresh-release")\\cozy-installer.ps1'" > "$OUT/fresh-windows.txt" 2>&1 \
    || check "fresh: Windows: the installer failed: $(tail -c 400 "$OUT/fresh-windows.txt")"
  [ -f "$dll" ] || check "fresh: Windows: no WinFsp after the install: $(tail -c 400 "$OUT/fresh-windows.txt")"
  grep -q "ADDLOCAL=ALL\|winfsp: without it" "$OUT/fresh-windows.txt" || check "fresh: Windows: the installer didn't install WinFsp itself"
  cozy --json doctor > "$OUT/fresh-doctor.json" 2> /dev/null || true
  for row in winfsp vcruntime git ffmpeg uv; do
    jq -e --arg row "$row" '[.tools[] | select(.name == $row and .found)] | length == 1' "$OUT/fresh-doctor.json" > /dev/null \
      || check "fresh: Windows: doctor doesn't find $row after the install"
  done
}

# macOS: the installer fetches ffmpeg, so the first thumbnail fetches nothing.
fresh_macos() {
  step "fresh: macOS: ffmpeg fetched by the install, the first thumbnail fetches nothing"
  local before after pic hex
  rm -rf "$HOME/.cozy/tools/ffmpeg"
  # No ffmpeg on PATH, as on a person's Mac: the pinned one is what's fetched.
  env PATH=/usr/bin:/bin:/usr/sbin:/sbin COZY_DOWNLOAD_URL="$FRESH_URL" sh "$OUT/fresh-release/cozy-installer.sh" < /dev/null > "$OUT/fresh-macos.txt" 2>&1 \
    || check "fresh: macOS: the installer failed: $(tail -c 400 "$OUT/fresh-macos.txt")"
  ls "$HOME"/.cozy/tools/ffmpeg/*/ffmpeg > /dev/null 2>&1 || { check "fresh: macOS: no ffmpeg in ~/.cozy/tools after the install: $(tail -c 400 "$OUT/fresh-macos.txt")"; return 0; }
  fetches() { cat "$HOME"/.cozy/calcifer/calcifer.log* 2> /dev/null | grep -c "fetching ffmpeg" || true; }
  before=$(fetches)
  pic=$(picture dog1.jpg)
  project_import "$RUN_PROJECT" "$pic" fresh_dog > /dev/null || { check "fresh: macOS: the picture wasn't imported"; return 0; }
  hex=$(asset_of "$RUN_PROJECT" fresh_dog | jq -r '.sha256 // empty')
  hex=${hex#sha256:}
  project_api "$RUN_PROJECT" "/thumbs/$hex" -o /dev/null || true
  for _ in $(seq 60); do [ -z "$(find "$RUN_PROJECT/.thumbs" -name "$hex*" 2> /dev/null)" ] || break; sleep 0.5; done
  [ -n "$(find "$RUN_PROJECT/.thumbs" -name "$hex*" 2> /dev/null)" ] || check "fresh: macOS: no thumbnail was drawn"
  after=$(fetches)
  [ "$after" = "$before" ] || check "fresh: macOS: the first thumbnail fetched ffmpeg"
}
