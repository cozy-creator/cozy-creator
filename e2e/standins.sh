# e2e/standins.sh, sourced: a Hub's models as an admin mirrors them, and a stand-in for each default a package declares that
# the Hub holds no release of (th-412: production holds each public, and the Hub refuses a package whose default isn't). The
# caller gives `seed` (a folder for its files), `log`, and `cozy` (on PATH or a function), signed in as the Hub's admin.
STANDINS_E2E=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# mirror <source> <org/name> <lane> <release> <page> [upload flags...]: a model uploaded by this computer's machine and
# published, as an admin mirrors an author's model.
mirror() {
  local name=${2//\//-} commit
  cozy --json model upload "$1" "$2" --lane "$3" --mirror "$5" --on local -y "${@:6}" > "$seed/upload-$name.json" 2>&1 \
    || { echo "uploading $2 failed: $(tail -c 600 "$seed/upload-$name.json")" >&2; return 1; }
  commit=$(python3 "$STANDINS_E2E/model-details.py" "$2" | jq -r '.checkpoints[0].checkpoint // empty')
  [ -n "$commit" ] || { echo "$2 has no commit after its upload" >&2; return 1; }
  cozy --json model publish "$2" "$4" "$commit" -y > "$seed/publish-$name.json" 2>&1 \
    || { echo "publishing $2 $4 failed: $(tail -c 600 "$seed/publish-$name.json")" >&2; return 1; }
  log "published $2@$4 ($3, $commit)"
}

# standin_defaults <package dir>: each slot default of its functions, internal and needs-ruled ones too (its interface as cozy
# describes it, as release-gate/defaults.sh reads it), of which this Hub holds no release, published as 1 MiB of random weights:
# production holds each public, and this Hub refuses a package whose default isn't (th-412). Every slot is a model's.
standin_defaults() {
  local described=$seed/describe-${1##*/}.json defaults r
  cozy --json --full run "$1" --describe > "$described" 2> "$described.err" \
    && defaults=$(jq -r '[.interface.entrypoints[].models[]?.default // empty] | unique | join(" ")' "$described") \
    || { echo "describing ${1##*/} for its defaults failed: $(tail -c 600 "$described") $(tail -c 300 "$described.err")" >&2; return 1; }
  for r in $defaults; do
    [ "$(cozy --json model show "$r" 2> /dev/null | jq -r '.error.kind // empty' 2> /dev/null)" = not_found ] || continue
    [ -f "$seed/standin.safetensors" ] || random_weights "$seed/standin.safetensors" 1 standin
    mirror "$seed/standin.safetensors" "${r%@*}" bf16 "${r#*@}" https://github.com/cozy-creator/cozy-creator/tree/master/e2e \
      --source-format as-is > /dev/null || return 1
    log "published $r, a stand-in for ${1##*/}'s default"
  done
  rm -f "$seed/standin.safetensors"
}

# random_weights <file> <MiB> [part]: one F16 tensor per 64 MiB of random finite values, written as it is made: no chunk repeats.
# With `part`, its metadata names it: a config the Runtime constructs from.
random_weights() {
  python3 -I - "$1" "$2" "${3:-}" <<'PY'
import json, os, struct, sys
path, mib, part = sys.argv[1], int(sys.argv[2]), sys.argv[3]
FINITE = bytes(b & 0xBF for b in range(256))  # a high byte without its exponent's top bit: no Inf or NaN
size, tensors = 64 << 20, -(-int(sys.argv[2]) // 64)
header, offset = {}, 0
for i in range(tensors):
    n = min(size, (mib << 20) - offset)
    header[f"w.{i}"] = {"dtype": "F16", "shape": [n // 2], "data_offsets": [offset, offset + n]}
    offset += n
if part:
    header["__metadata__"] = {"part": part}
raw = json.dumps(header).encode()
raw += b" " * (-len(raw) % 8)
with open(path, "wb") as out:
    out.write(struct.pack("<Q", len(raw)) + raw)
    left = offset
    while left:
        chunk = bytearray(os.urandom(min(left, 8 << 20)))
        chunk[1::2] = chunk[1::2].translate(FINITE)
        out.write(chunk)
        left -= len(chunk)
PY
}
