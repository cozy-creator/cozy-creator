#!/usr/bin/env bash
# The `cli-release` suite: cozy's signed-in commands before a release (e2e/coverage.toml's `release` tier), on Linux,
# macOS or Windows. Signed in with the CI key (or, by hand with --installed, as this computer is), it reads the
# account's credit and makes (never pays) a checkout, then rents ONE machine of --sku (default: the catalog's cheapest
# GPU; nothing dearer than --max-hourly an hour). A throwaway fidika/ci-e2e-<run> package and model go through lock,
# publish, yank, un-yank and delete, run on this computer's machine or, where the OS runs none (macOS, Windows), on the
# rental. On the rental: package install and list, BiRefNet from a YAML request and with --asset, each output landed
# here (one in a folder named with spaces and non-ASCII), machine list, show and logs, keepalive, a run followed and
# canceled, and an end while a run still goes, checked gone. The throwaway and the rental are removed in an EXIT trap
# whatever fails. --quote stops at the rental's quote and rents nothing: the $0 pre-flight of every step before it.
#   e2e/release/run.sh --out DIR [--key FILE | --installed] [--sku SKU] [--max-hourly USD] [--quote] [--release DIR]
#                      [--tensord F|V] [--tensorfs F|V] [--runtime F|V]
. "$(dirname "$0")/../lib.sh"
parse "$@"
here=$(cd "$(dirname "$0")" && pwd)
J() { jq -e "$@" > /dev/null; }
run=$(date -u +%m%d%H%M)$RANDOM
pkg=ci-e2e-$run model=fidika/ci-e2e-$run M=''

# The machine an answer of `machine rent` names (pretty or NDJSON).
rented() { jq -r '.. | objects | select(.kind? == "rental") | .name // empty' "$1" 2> /dev/null | tail -1; }
# Whether `machine list` lists machine $1.
listed() { cozy --json machine list | J --arg m "$1" '.machines | any(.name == $m)'; }
cleanup() {
  set +e
  [ -n "$M" ] || [ ! -s "$OUT/rent.ndjson" ] || M=$(rented "$OUT/rent.ndjson")
  # `machine end` returns once the Hub affirms the pod gone.
  if [ -n "$M" ]; then
    cozy --json machine end "$M" --force -y > "$OUT/end-trap.json" 2>&1
    ! listed "$M" || echo "::error::the trap ended $M, but machine list still lists it: end it by hand"
  fi
  cozy --json package delete "fidika/$pkg" -y > /dev/null 2>&1
  cozy --json model delete "$model" -y > /dev/null 2>&1
}
trap cleanup EXIT
# A runner's cancel or timeout signals the script: it exits, so the trap ends the rental.
trap 'exit 1' INT TERM HUP
usd() { awk -v m="$1" 'BEGIN { printf "%.2f", m / 1e6 }'; }

# watched <name> <cozy args...>: a verb that makes a numbered run, followed to its end by the CLI's own events (run.sh);
# its events in $OUT/<name>.ndjson, its record in <name>.json. 0: it completed.
watched() {
  local name=$1; shift
  [ -n "$(run_submit "$OUT" "$name" "$@")" ] && run_wait "$OUT" "$name" && [ "$(run_get "$OUT" "$name" .status)" = completed ]
}

if [ -z "$INSTALLED" ]; then step install; install_cozy; user_services; step candidates; candidates; fi

step sign-in
if [ -n "$KEY" ]; then cozy --json login --key "$KEY" > "$OUT/login.json" || fail "cozy login --key failed"; fi
cozy --json whoami > "$OUT/whoami.json"
J '.logged_in == true and .account == "fidika"' "$OUT/whoami.json" || fail "whoami: not signed in as fidika ($(head -c 300 "$OUT/whoami.json"))"

step credits
cozy --json credits > "$OUT/credits.json" && J '.billing | length > 0' "$OUT/credits.json" || check "credits: no billing"
cozy --json credits history > "$OUT/history.json" || check "credits history failed"
J 'type == "object"' "$OUT/history.json" || check "credits history: no answer"
cozy --json credits buy 10 --no-browser --no-wait > "$OUT/buy.json" || check "credits buy 10 (the least one purchase may be) failed"
checkout=$(jq -r '.. | strings | select(test("^https://"))' "$OUT/buy.json" | head -1) || true
[ -n "$checkout" ] && curl -fsS -o /dev/null "$checkout" || check "credits buy: no payment page ($(head -c 300 "$OUT/buy.json"))"

step "the rental's quote"
[ -n "$SKU" ] || SKU=$(cozy --json machine rent | jq -r '[.[] | select(.vram_gb > 0)][0].name // empty') || true
[ -n "$SKU" ] || fail "machine rent: the catalog lists no GPU"
code=0; cozy --json machine rent "$SKU" --disk-gb 60 > "$OUT/quote.ndjson" 2>&1 || code=$?
hourly=$(jq -r 'select(.quote) | .quote.usd_micros_per_hour' "$OUT/quote.ndjson" 2> /dev/null | head -1) || true
[ "$code" = 2 ] && grep -q quote_unagreed "$OUT/quote.ndjson" && [ -n "$hourly" ] \
  || fail "machine rent $SKU without --yes: exit $code, not its quote alone: $(head -c 400 "$OUT/quote.ndjson")"
awk -v h="$hourly" -v max="$MAX_HOURLY" 'BEGIN { exit !(h / 1e6 <= max) }' || fail "machine rent $SKU: \$$(usd "$hourly")/h, over --max-hourly \$$MAX_HOURLY"
echo "quote: $SKU at \$$(usd "$hourly")/h"
if [ -n "$QUOTE" ]; then
  ! listed "ci-e2e-quote-$run" || check "machine list: the gone check by name lists a machine never rented"
  finish "cli-release --quote: $SKU at \$$(usd "$hourly")/h, nothing rented"
  exit 0
fi

step rental
before=$(cozy --json machine list | jq '[.machines[] | select(.kind == "rental")] | length') || true
! cozy --json machine rent "$SKU" --gpus 64 -y > "$OUT/rent-too-many.json" 2>&1 || check "machine rent --gpus 64 bought something"
[ "$(cozy --json machine list | jq '[.machines[] | select(.kind == "rental")] | length')" = "$before" ] || check "machine rent --gpus 64: a rental appeared"
cozy --json machine rent "$SKU" --disk-gb 60 -y > "$OUT/rent.ndjson" || fail "machine rent $SKU failed: $(tail -c 400 "$OUT/rent.ndjson")"
M=$(rented "$OUT/rent.ndjson") || true
[ -n "$M" ] || fail "machine rent: no machine named"
echo "rented $M ($SKU)"
cozy --json machine show "$M" > "$OUT/rental.json" && J '.rental.disk_gb >= 60' "$OUT/rental.json" || check "machine rent --disk-gb 60: $(jq .rental.disk_gb "$OUT/rental.json") GB"
cozy --json machine list | J --arg m "$M" '.machines | any(.name == $m and .kind == "rental")' || check "machine list does not list $M"
cozy machine logs "$M" --lines 5 > "$OUT/rental-log.txt" && [ -s "$OUT/rental-log.txt" ] || check "machine logs $M --lines 5: nothing"
# Where this OS runs no models itself, the throwaway runs on the rental.
ON=$M
[ -z "$LOCAL" ] || ON=local

step "throwaway package"
dir=$OUT/$pkg
cp -r "$here/../cli/weightless" "$dir"
sed -i.orig -e "s/^name = \"e2e-weightless\"/name = \"$pkg\"/" "$dir/pyproject.toml" && rm -f "$dir/pyproject.toml.orig"
(cd "$dir" && cozy --json package lock > "$OUT/lock.json") && [ -s "$dir/uv.lock" ] || check "package lock: no uv.lock"
(cd "$dir" && cozy --json package lock --upgrade-package msgspec > "$OUT/lock-upgrade.json") && grep -q 'name = "msgspec"' "$dir/uv.lock" \
  || check "package lock --upgrade-package msgspec: msgspec not locked"
(cd "$dir" && cozy --json package publish > "$OUT/publish-1.json") || check "package publish 0.1.0 failed"
sed -i.orig 's/^version = "0.1.0"/version = "0.2.0"/' "$dir/pyproject.toml" && rm -f "$dir/pyproject.toml.orig"
(cd "$dir" && cozy --json package lock > /dev/null && cozy --json package publish > "$OUT/publish-2.json") || check "package publish 0.2.0 failed"
cozy --json package info "fidika/$pkg" > "$OUT/pkg-info.json" && J '[.releases[].release] | index("0.1.0") and index("0.2.0")' "$OUT/pkg-info.json" \
  || check "package info: not both releases"
resolves() {  # the release an unversioned run of the throwaway takes
  watched "resolve-$1" run "fidika/$pkg/reverse" text=e2e --on "$ON" || true
  grep -oE "\"target\": *\"[^\"]*$pkg[^\"]*\"" "$OUT/resolve-$1.ndjson" | head -1 || true
}
[[ $(resolves before) == *@0.2.0* ]] || check "the throwaway's unversioned run is not 0.2.0"
cozy --json package yank "fidika/$pkg@0.2.0" > "$OUT/yank.json" || check "package yank failed"
[[ $(resolves yanked) == *@0.1.0* ]] || check "package yank: an unversioned run still takes 0.2.0"
cozy --json package yank "fidika/$pkg@0.2.0" --undo > "$OUT/unyank.json" || check "package yank --undo failed"
[[ $(resolves unyanked) == *@0.2.0* ]] || check "package yank --undo: an unversioned run does not take 0.2.0 again"
cozy --json package delete "fidika/$pkg" -y > "$OUT/pkg-delete.json" || check "package delete failed"
! cozy --json package info "fidika/$pkg" > /dev/null 2>&1 || check "package delete: fidika/$pkg still answers"

step "throwaway model"
uv run -q --no-project python -I - "$OUT/tiny.safetensors" <<'PY'
import json, struct, sys
header = json.dumps({"w": {"dtype": "F16", "shape": [4, 4], "data_offsets": [0, 32]}}).encode()
header += b" " * (-len(header) % 8)
open(sys.argv[1], "wb").write(struct.pack("<Q", len(header)) + header + bytes(32))
PY
watched upload model upload "$OUT/tiny.safetensors" "$model" --lane f16 --on "$ON" || check "model upload failed"
commit=$(jq -r '.. | objects | .commit? // empty' "$OUT/upload.ndjson" 2> /dev/null | tail -1) || true
uv run -q --no-project python -I - "$OUT/tiny-b.safetensors" <<'PY'
import json, struct, sys
header = json.dumps({"w": {"dtype": "F32", "shape": [4, 4], "data_offsets": [0, 64]}}).encode()
header += b" " * (-len(header) % 8)
open(sys.argv[1], "wb").write(struct.pack("<Q", len(header)) + header + bytes(64))
PY
watched upload-b model upload "$OUT/tiny-b.safetensors" "$model" --lane f32 --on "$ON" || check "model upload --lane f32 failed"
commit_b=$(jq -r '.. | objects | .commit? // empty' "$OUT/upload-b.ndjson" 2> /dev/null | tail -1) || true
cozy --json model family e2e-family "$model" > /dev/null && cozy --json model info "$model" | J '.family == "e2e-family"' || check "model family: not set"
cozy --json model family --clear "$model" > /dev/null && cozy --json model info "$model" | J '.family == null' || check "model family --clear: not cleared"
cozy --json model publish "$model" 0.1.0 "$commit" "$commit_b" > "$OUT/model-publish.json" || check "model publish failed"
cozy --json model info "$model" > "$OUT/model-info.json" \
  && J '.releases | any(.version == "0.1.0" and ([.lanes[].lane] | sort) == ["f16", "f32"])' "$OUT/model-info.json" \
  || check "model publish: no 0.1.0 with lanes f16 and f32"
cozy --json model delete "$model" -y > "$OUT/model-delete.json" || check "model delete failed"
! cozy --json model info "$model" > /dev/null 2>&1 || check "model delete: $model still answers"

step "BiRefNet on $M"
watched install-birefnet package install birefnet --on "$M" \
  || check "package install birefnet --on $M failed: $(run_get "$OUT" install-birefnet '.reason.message' | head -c 300)"
cozy --json package list --on "$M" > "$OUT/packages-rental.json" && grep -q birefnet "$OUT/packages-rental.json" || check "package list --on $M: no birefnet"
# A request file beside its image, in a folder named with spaces and non-ASCII; its output lands in another such folder.
far="$OUT/in dir é ü" landing="$OUT/out dir é ü"
mkdir -p "$far" && cp "$(picture dog1.jpg)" "$far/chien é.jpg" && printf 'image: chien é.jpg\n' > "$far/request é.yaml"
if watched birefnet-yaml run birefnet/remove_background --input "$far/request é.yaml" --on "$M" --out "$landing"; then
  out=$(run_output "$OUT" birefnet-yaml webp)
  [[ $out == *"out dir é ü"* ]] && [ -s "$out" ] || check "birefnet-yaml: its output did not land in '$landing' (${out:-none})"
  white_background "$out" "$far/chien é.jpg" || check "birefnet-yaml: not a background removal"
else check "birefnet-yaml: cozy run birefnet/remove_background --input --on $M did not complete: $(run_get "$OUT" birefnet-yaml '.reason.message' | head -c 300)"; fi
dog=$(picture dog2.jpg)
if run_cell "$OUT" birefnet-asset birefnet/remove_background --asset "image=$dog" --on "$M"; then
  white_background "$(run_output "$OUT" birefnet-asset webp)" "$dog" || check "birefnet-asset: not a background removal"
else check "birefnet-asset: cozy run --asset --on $M did not complete: $(run_get "$OUT" birefnet-asset '.reason.message' | head -c 300)"; fi
cozy --json run list --on "$M" > "$OUT/runs-rental.json" && J --arg m "$M" '(.runs | length > 0) and all(.runs[]; .machine == $m)' "$OUT/runs-rental.json" \
  || check "run list --on $M: other machines' runs, or none"

step "a run on $M followed, then canceled"
deadline=$(jq '.idle_deadline_ms // 0' "$OUT/rental.json") || true
cozy --json machine keepalive "$M" > "$OUT/keepalive.json" || check "machine keepalive failed"
J --argjson d "${deadline:-0}" '.idle_deadline_ms > $d' "$OUT/keepalive.json" || check "machine keepalive: its idle deadline did not move"
(cd "$here/../cli" && watched on-rental run ./weightless/reverse text=rented --on "$M") && grep -q detner "$OUT/on-rental.ndjson" \
  || check "a run on $M: no reply"
n=$(cd "$here/../cli" && run_submit "$OUT" cancel-wait run ./weightless/wait seconds:=300 --on "$M")
if [ -z "$n" ] || ! run_wait "$OUT" cancel-wait in_progress; then check "the waiting run on $M never ran"; else
  cozy --json run show "$n" > "$OUT/cancel-followed.ndjson" &
  following=$!
  cozy --json run cancel "$n" > "$OUT/cancel.json" || check "run cancel $n failed"
  wait "$following" || true
  grep -qE '"status": *"canceled"' "$OUT/cancel-followed.ndjson" || check "run show $n did not follow it to canceled"
fi
run_wait "$OUT" cancel-wait || true

step "an end while a run goes"
(cd "$here/../cli" && cozy --json run ./weightless/wait seconds:=600 --on "$M" > "$OUT/rental-wait.ndjson" 2>&1) &
until run_reached "$OUT/rental-wait.ndjson" in_progress || ! kill -0 $! 2> /dev/null; do sleep 0.2; done
# `machine end` returns once the Hub affirms the pod gone.
cozy --json machine end "$M" --force -y > "$OUT/end.json" || check "machine end $M --force failed"
! listed "$M" && M='' || check "machine end --force: $M is still listed"
wait || true

step coverage; coverage "$COZY" release
finish "cli-release: $pkg, $model, ${SKU} at \$$(usd "$hourly")/h"
