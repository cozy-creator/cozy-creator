# no_traces: Paul's invariant on this runner's OS (cl-984). An encrypted project made, given a box with a marker prompt and an import (and a run of the prompt where this computer runs tools), then locked, leaves
# nothing of itself outside its folder (containment/notraces.py): containment's lock scans, portable to macOS and Windows.
notraces_here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
no_traces() {
step "no traces: an encrypted project run with a marker prompt and locked leaves nothing of itself outside its folder"
local root=$OUT/notraces
mkdir -p "$root"
(cd "$notraces_here/containment" && uv run --no-project notraces.py --cli "$COZY" --root "$root" --out "$OUT") > "$OUT/notraces.out" 2>&1 \
  || check "no traces: $(tail -c 400 "$OUT/notraces.out")"
echo "no traces: $(tail -1 "$OUT/notraces.out")"
}
