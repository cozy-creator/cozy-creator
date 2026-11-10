# A --cells hook (gate.sh --cells) for a machine change on one GPU, beside any suite, with --only naming its cells:
#   xs-081, one executor protocol: sdxl/generate with a LoRA (an adapter view), then sdxl/image of the same release.
#   cl-843, one content store: a weights output read after TensorD restarts (`model upload run:N#model`), and a model
#   upload to the machine cut a quarter in by killing this computer's Calcifer, then sent on from what the machine holds.
#   A cancel does not cut it: the canceled run's upload goes on and the next run of the same file waits for it (42752980037).
# A gate runs the release-style TensorFS wheel: e2e's candidate wheel has no Torch allocator shim, so cli-release cannot
# run GPU work on a candidate TensorFS.
#   gh workflow run release-gate.yml -f suites=packages -f cells=release-gate/suites/tensord/pod-cells.sh \
#     -f only=pc-lora,pc-image,pc-held-source,pc-resume -f pins=sdxl@<candidate> ...
want() { [ -z "$ONLY" ] || [[ $ONLY == *",$1,"* ]]; }
cell pc-lora "$(pkg sdxl generate)" prompt="$OBJECT as flat-pack furniture, catalog photo" seed:=$((R + 70)) --lora ostris/ikea-lora-sdxl
if [ -f pc-lora.json ]; then
  [ "$(run_get "$OUT" pc-lora .status)" = completed ] && grep -q ikea-lora-sdxl pc-lora.json && [ -s "$(output pc-lora 'png|webp|jpg')" ] \
    && expect pc-lora-adapter PASS "run $(cat pc-lora.number): an image, its record names the LoRA" \
    || expect pc-lora-adapter FAIL "run $(cat pc-lora.number) $(run_get "$OUT" pc-lora .status): $(run_get "$OUT" pc-lora .reason.message | head -c 300)"
fi
cell pc-image "$(pkg sdxl image)" instructions="$SCENE, oil painting" seed:=$((R + 71))

WHO=$("$CLI" --json whoami 2> /dev/null | jq -r '.account // empty') dest=''
[ -z "$WHO" ] || dest=$WHO/ci-pc-$R
if want pc-held-source; then
ln -sfn "$HERE/suites/tensorfs/multi-conversion" "$OUT/ci-multi-conversion"
cell pc-held-source ./ci-multi-conversion/source seed:=$((R + 72))
was=$("$CLI" --json machine show "$M" | jq -r '.status.started_at_ms // empty')
"$CLI" machine update "$M" --gpu-memory-limit off -y > pc-restart.log 2>&1
now=$("$CLI" --json machine show "$M" | jq -r '.status.started_at_ms // empty')
[ -n "$now" ] && [ "$now" != "$was" ] && expect pc-restart PASS "TensorD started at $was, then $now" \
  || expect pc-restart FAIL "TensorD did not restart ($was, then $now): $(tail -c 300 pc-restart.log)"
if [ "$(run_get "$OUT" pc-held-source .status)" = completed ] && [ -n "$dest" ]; then
  src=$(cat pc-held-source.number)
  run_submit "$OUT" pc-held-upload model upload "run:$src#model" "$dest" --lane bf16 --on "$M" -y > /dev/null; run_wait "$OUT" pc-held-upload
  [ "$(run_get "$OUT" pc-held-upload .status)" = completed ] && expect pc-held PASS "run $src's weights output uploaded after the restart (run $(cat pc-held-upload.number))" \
    || expect pc-held FAIL "run $src's output after the restart: $(run_get "$OUT" pc-held-upload .reason.message | head -c 300)"
fi
fi

if want pc-resume && [ -n "$dest" ]; then
  python3 -I - "$OUT/pc-big.safetensors" <<'PY'
import json, os, struct, sys
n = 256 << 20
header = json.dumps({"w": {"dtype": "F16", "shape": [n // 2], "data_offsets": [0, n]}}).encode()
header += b" " * (-len(header) % 8)
with open(sys.argv[1], "wb") as f:
    f.write(struct.pack("<Q", len(header)) + header)
    for _ in range(n >> 24):
        f.write(os.urandom(1 << 24))
PY
  lock=$HOME/.cozy/calcifer/calcifer.lock.holder cut=0
  n=$(run_submit "$OUT" pc-cut model upload "$OUT/pc-big.safetensors" "$dest" --lane f16-c --source-format as-is --on "$M" -y)
  while [ -n "$n" ] && kill -0 "$(cat pc-cut.pid)" 2> /dev/null; do
    read -r _ most total < <(run_uploaded pc-cut.ndjson)
    if [ "$total" -gt 0 ] && [ "$most" -ge $((total / 4)) ]; then kill -9 "$(jq -r .pid "$lock")"; cut=$most; break; fi
    sleep 0.3
  done
  while kill -0 "$(cat pc-cut.pid)" 2> /dev/null; do sleep 0.2; done
  # A service manager or the next command starts Calcifer again; it drives the cut run on, and the follower goes on with
  # it. Each send of a file says where it begins with no rate (`uploading <x>: <held>/<total>`): the second such line is
  # the send after the cut, which must begin at what the machine holds (42756795346: 73.3 of 256 MiB). A run Calcifer
  # ended instead is sent again, and its first line says the same.
  "$CLI" --json machine show "$M" > /dev/null 2>&1
  run_show "$OUT" pc-cut "$n"; events=pc-cut.ndjson
  if [ "$(run_get "$OUT" pc-cut .status)" != completed ]; then
    run_submit "$OUT" pc-resume model upload "$OUT/pc-big.safetensors" "$dest" --lane f16-c --source-format as-is --on "$M" -y > /dev/null
    run_wait "$OUT" pc-resume; events=pc-resume.ndjson
  fi
  begins=$(grep '^{' "$events" | jq -r 'select(.type? == "run.waiting") | .reason // "" | select(test("^uploading .*: [0-9.]+ [KMGT]?i?B/[0-9.]+ [KMGT]?i?B$"))' | sed -E 's/.*: ([^/]+)\/.*/\1/')
  from=$([ "$events" = pc-cut.ndjson ] && sed -n 2p <<< "$begins" || sed -n 1p <<< "$begins")
  [ "$cut" -gt 0 ] && [ -n "$from" ] && [[ $from != "0 B" ]] && [ "$(run_get "$OUT" "${events%.ndjson}" .status)" = completed ] \
    && expect pc-resume PASS "Calcifer killed at $cut bytes; the send after began at $from of the file ($events), and completed" \
    || expect pc-resume FAIL "killed at $cut; the send after began at ${from:-nothing} ($events): $(grep -o '"reason":"uploading[^"]*"' "$events" | grep -v ' at ' | head -3 | tr '\n' ' ')"
  rm -f "$OUT/pc-big.safetensors"
  "$CLI" --json model delete "$dest" -y > pc-delete.json 2>&1 || log "model delete $dest: $(tail -c 200 pc-delete.json)"
fi
