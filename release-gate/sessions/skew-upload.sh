# skew-upload (cl-1195): local code sent across RC builds both ways, on a rental of $SKU, this runner's Calcifer sending it. cr-326's
# 4-GPU try failed `invalid_range` on a part an older Calcifer sent its newer TensorD. Each half installs a package of its own whose
# folder holds an empty file, a one-byte file and 300 small ones, then runs its function: first this runner on the `old` cozy and the
# rental's TensorD this session's, then the reverse. Each completes, and its upload is said in its run (`uploading …`).
# Without `rent`, $0: the quote, and the checks before it.
# params:
#   old      a cozy-creator dev-build run id of the other build (required)
#   runtime  a cozy-runtime dev-build run id the rental runs (default: none, the RC's)
#   files    small files a package holds (default 300)
#   seconds  what the cap must buy (default 1500)
. "$(dirname "$0")/lib.sh"
check() { say "FAIL $*"; RC=1; }
for tool in jq python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no dev build: dispatch with cozy=<a dev-build run of the change>"
OLD=$(param old '') SRC=$(param runtime '') FILES=$(param files 300)
[[ $OLD =~ ^[0-9]+$ ]] || fail "old: a cozy-creator dev-build run id"
COZY_BIN=$(command -v cozy)
cp "$COZY_BIN" "$OUT/new-cozy" || fail "no copy of this session's cozy"
gh run download "$OLD" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$OUT/old-dl" > /dev/null 2>&1 || fail "no cozy in dev-build run $OLD"
install -m 0755 "$(find "$OUT/old-dl" -name cozy-linux-x86_64 | head -1)" "$OUT/old-cozy" || fail "no cozy-linux-x86_64 in run $OLD"
say "old: $("$OUT/old-cozy" --version), new: $(cozy --version)"
W=''
if [ -n "$SRC" ]; then
  [[ $SRC =~ ^[0-9]+$ ]] || fail "runtime: a cozy-runtime dev-build run id"
  W=$(wheel_from "$OUT/wheel" "$SRC") && [ -n "$W" ] || fail "no manylinux wheel in cozy-runtime run $SRC"
fi
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
[ -n "$ON" ] || [ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
trap 'code=$?; claim; (exit "$code"); finish' EXIT
quote "$(param seconds 1500)"
GUARD_ALSO=own guard
rent
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done

# package <dir> <label>: a one-function package whose folder holds an empty file, a one-byte file and FILES small ones, each its label's.
package() {
  mkdir -p "$1/data"
  cat > "$1/pyproject.toml" <<TOML
[project]
name = "skew-$2"
version = "0.0.0"
requires-python = ">=3.12"
dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19"]

[project.entry-points."cozy.application"]
default = "words:app"

[build-system]
requires = ["hatchling>=1.25"]
build-backend = "hatchling.build"

[tool.hatch.build.targets.wheel]
only-include = ["words.py"]
TOML
  cat > "$1/words.py" <<'PY'
import msgspec

from cozy_runtime.author import App

app = App()


class Words(msgspec.Struct, forbid_unknown_fields=True):
    text: str


class Counted(msgspec.Struct):
    words: int


@app.entrypoint(accelerator=False)
def count(payload: Words) -> Counted:
    return Counted(words=len(payload.text.split()))
PY
  : > "$1/data/empty.txt"
  printf x > "$1/data/tiny.txt"
  python3 -I -c 'import sys, pathlib
d, label, n = pathlib.Path(sys.argv[1]), sys.argv[2], int(sys.argv[3])
for i in range(n): (d / f"f{i:03d}.txt").write_text(f"{label} {i} " * (20 + i % 60))' "$1/data" "$2" "$FILES"
}

# half <label> <calcifer cozy> <tensord cozy>
half() {
  local label=$1 calcifer=$2 tensord=$3 pkg=$OUT/pkg-$1
  if [ "$(cozy --version)" != "$("$calcifer" --version)" ]; then
    install -m 0755 "$calcifer" "$COZY_BIN" && cozy restart > "$OUT/restart-$label.txt" 2>&1 || fail "$label: cozy restart: $(tail -c 300 "$OUT/restart-$label.txt")"
  fi
  local update=(--tensord "$tensord")
  [ -z "$W" ] || update+=(--runtime "$W")
  step "update-$label" machine update "$NAME" "${update[@]}" -y || fail "$label: the pod's TensorD"
  for _ in $(seq 180); do
    cozy machine show "$NAME" --json > "$OUT/show-$label.json" 2> /dev/null
    [ "$(jq -r '.state // empty' "$OUT/show-$label.json")" = ready ] && break
    sleep 5
  done
  say "$label: Calcifer $(cozy --version | awk '{print $2}'), TensorD $(jq -r '.status.version' "$OUT/show-$label.json")"
  package "$pkg" "$label"
  if (cd "$RUN_PROJECT" && cozy run "$pkg" "text=one two three" --on "$NAME") > "$OUT/$label.txt" 2>&1; then
    say "$label: installed and ran: $(grep -c 'uploading' "$OUT/$label.txt") upload lines, $(grep -E 'words|status' "$OUT/$label.txt" | tr -s ' ' | paste -sd' ' | cut -c1-200)"
  else
    check "$label: $(tail -n 4 "$OUT/$label.txt" | paste -sd' ' | cut -c1-400)"
  fi
}
half old-to-new "$OUT/old-cozy" "$OUT/new-cozy"
half new-to-old "$OUT/new-cozy" "$OUT/old-cozy"
cozy machine logs "$NAME" tensord --since 2h -n 0 > "$OUT/tensord.log" 2>&1
{ echo "## skew-upload on $NAME ($SKU)"; grep -E 'installed and ran|FAIL' "$R" | sed 's/^/- /'; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
