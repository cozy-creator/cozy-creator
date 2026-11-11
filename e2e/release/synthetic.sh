#!/usr/bin/env bash
# Sourced only by e2e/skew/run.sh --checks, after its isolated Hub sign-in.
# All repository writes and their cleanup stay in that disposable environment.
release_synthetic_checks() (
  local here E2E=${E2E:?} ROOT_OUT=${OUT:?} COZY=${COZY_BIN:-${COZY:-}}
  [ -n "$COZY" ] || COZY=$(type -P "${CLI:-cozy}") || return 1
  here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  . "$E2E/test-hub.sh"
  require_test_hub "${COZY_HUB_URL:?}" || return 1
  local OUT=$ROOT_OUT/release-publication run pkg model M=local ON=local
  run=$(date -u +%m%d%H%M)$RANDOM
  pkg=ci-e2e-$run model=${WHO:?}/ci-e2e-$run
  mkdir -p "$OUT"
  [ -z "${PUBLICATION_LOG:-}" ] || mkdir -p "$(dirname "$PUBLICATION_LOG")" || return 1
  J() { jq -e "$@" > /dev/null; }
  check() { expect "release publication" FAIL "$*"; }
  step() { log "release publication: $*"; }
  cozy() {
    printf '%s\x1f' "$@" >> "$OUT/argv.log"; printf '\x1e' >> "$OUT/argv.log"
    if [ -n "${PUBLICATION_LOG:-}" ] && [ "$PUBLICATION_LOG" != "$OUT/argv.log" ]; then
      printf '%s\x1f' "$@" >> "$PUBLICATION_LOG"; printf '\x1e' >> "$PUBLICATION_LOG"
    fi
    "$COZY" "$@"
  }
  RUN_CLI=cozy
  . "$E2E/run.sh"
  watched() {
    local name=$1; shift
    [ -n "$(run_submit "$OUT" "$name" "$@")" ] && run_wait "$OUT" "$name" && [ "$(run_get "$OUT" "$name" .status)" = completed ]
  }
  cleanup_publication() {
    local status=$?
    step cleanup
    python3 "$E2E/delete-test-package.py" "$WHO/$pkg" > /dev/null || check "package fixture cleanup failed"
    for model_name in "$model" "$model-copy" "$model-store" "$model-lora"; do
      if model_details "$model_name" > /dev/null 2>&1; then
        cozy --json model delete "$model_name" -y > /dev/null || check "cleanup failed for $model_name"
      fi
    done
    return "$status"
  }
  trap cleanup_publication EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  run_project "$OUT/project" || return 1
step "throwaway package"
dir=$OUT/$pkg
cp -r "$here/../cli/weightless" "$dir"
# It publishes what a published package may hold: weightless's `orphan` forks on purpose (the executor cells' kill test), which
# a Runtime's describe refuses since 0.43.1 ("os.fork: a child process is outside the sandbox"; e2e 42762119315).
uv run -q --no-project python -I - "$dir/e2e_weightless/__init__.py" <<'PY'
import re, sys
path = sys.argv[1]
text = open(path).read()
open(path, "w").write(re.sub(r"@app\.entrypoint\(accelerator=False\)\ndef orphan\(.*?(?=\n@app\.)", "", text, flags=re.S))
PY
sed -i.orig -e "s/^name = \"e2e-weightless\"/name = \"$pkg\"/" "$dir/pyproject.toml" && rm -f "$dir/pyproject.toml.orig"
[ ! -e "$dir/uv.lock" ] || check "the package fixture already has a uv.lock"
(cd "$dir" && cozy --json package publish > "$OUT/publish-1.json") && [ -s "$dir/uv.lock" ] \
  || check "package publish 0.1.0 failed or did not create uv.lock"
cp "$dir/uv.lock" "$OUT/lock-before-bump.toml"
sed -i.orig 's/^version = "0.1.0"/version = "0.2.0"/' "$dir/pyproject.toml" && rm -f "$dir/pyproject.toml.orig"
(cd "$dir" && cozy --json package publish > "$OUT/publish-2.json") || check "package publish 0.2.0 failed"
uv run -q --no-project python -I - "$dir/uv.lock" "$pkg" "$OUT/lock-before-bump.toml" <<'PYLOCK' || check "package publish did not refresh the root version while preserving dependency pins"
import sys, tomllib
with open(sys.argv[1], "rb") as file:
    packages = tomllib.load(file)["package"]
assert any(package["name"] == sys.argv[2] and package["version"] == "0.2.0" for package in packages), "published lock has a stale root version"
with open(sys.argv[3], "rb") as file:
    before = tomllib.load(file)["package"]
def pins(packages):
    return {package["name"]: (package["version"], package["source"]) for package in packages if package["name"] != sys.argv[2]}
assert pins(before) == pins(packages), "unchanged dependencies moved during the version bump"
PYLOCK
package_details "$WHO/$pkg" > "$OUT/pkg-info.json" && J '[.releases[].release] | index("0.1.0") and index("0.2.0")' "$OUT/pkg-info.json" \
  || check "package details: not both releases"
resolves() {  # the release an unversioned run of the throwaway takes
  watched "resolve-$1" run "$WHO/$pkg/reverse" text=e2e --on "$ON" || true
  grep -oE "\"target\": *\"[^\"]*$pkg[^\"]*\"" "$OUT/resolve-$1.ndjson" | head -1 || true
}
[[ $(resolves before) == *@0.2.0* ]] || check "the throwaway's unversioned run is not 0.2.0"
cozy --json package yank "$WHO/$pkg@0.2.0" > "$OUT/yank.json" || check "package yank failed"
[[ $(resolves yanked) == *@0.1.0* ]] || check "package yank: an unversioned run still takes 0.2.0"
cozy --json package yank "$WHO/$pkg@0.2.0" --undo > "$OUT/unyank.json" || check "package yank --undo failed"
[[ $(resolves unyanked) == *@0.2.0* ]] || check "package yank --undo: an unversioned run does not take 0.2.0 again"
# A candidate: yanked in one Hub call, so the publisher's own next unversioned run keeps the newest (cl-732).
sed -i.orig 's/^version = "0.2.0"/version = "0.3.0"/' "$dir/pyproject.toml" && rm -f "$dir/pyproject.toml.orig"
(cd "$dir" && cozy --json package publish --yanked > "$OUT/publish-3.json") || check "package publish --yanked 0.3.0 failed"
J '.status == "published yanked"' "$OUT/publish-3.json" || check "package publish --yanked: $(jq -r .status "$OUT/publish-3.json")"
[[ $(resolves candidate) == *@0.2.0* ]] || check "package publish --yanked: the publisher's next unversioned run took the candidate"
# A release its machine still holds and the Hub no longer has, as after a renumbering (birefnet 0.5.3 over 0.1.3): never chosen (cl-748).
cozy --json package delete "$WHO/$pkg" --yes > "$OUT/pkg-renumber.json" || check "test package cleanup, to renumber, failed"
sed -i.orig 's/^version = "0.3.0"/version = "0.1.0"/' "$dir/pyproject.toml" && rm -f "$dir/pyproject.toml.orig"
(cd "$dir" && cozy --json package publish > "$OUT/publish-4.json") || check "package publish 0.1.0 again failed"
cozy --json package ls --on "$ON" | J --arg p "$WHO/$pkg" '.packages | any(.package == $p and .release == "0.2.0")' || check "the machine no longer holds 0.2.0, so this proves nothing"
[[ $(resolves renumbered) == *@0.1.0* ]] || check "an unversioned run took a release the Hub no longer has: $(tail -c 300 "$OUT/resolve-renumbered.err")"
[ "$(run_get "$OUT" resolve-renumbered .status)" = completed ] || check "an unversioned run, with a release the Hub no longer has held: $(run_get "$OUT" resolve-renumbered '.reason.message' | head -c 300)"
python3 "$here/../delete-test-package.py" "$WHO/$pkg" > "$OUT/pkg-delete.json" || check "test package cleanup failed"
! package_details "$WHO/$pkg" > /dev/null 2>&1 || check "test package cleanup: $WHO/$pkg still answers"

step "throwaway model"
uv run -q --no-project python -I - "$OUT/tiny.safetensors" <<'PY'
import json, os, struct, sys
header = json.dumps({"w": {"dtype": "F16", "shape": [4, 4], "data_offsets": [0, 32]}}).encode()
header += b" " * (-len(header) % 8)
open(sys.argv[1], "wb").write(struct.pack("<Q", len(header)) + header + os.urandom(32))
PY
watched upload model upload "$OUT/tiny.safetensors" "$model" --lane f16 --source-format as-is --on "$ON" || check "model upload failed"
commit=$(jq -r '.. | objects | .commit? // empty' "$OUT/upload.ndjson" 2> /dev/null | tail -1) || true
uv run -q --no-project python -I - "$OUT/tiny-b.safetensors" <<'PY'
import json, os, struct, sys
header = json.dumps({"w": {"dtype": "F32", "shape": [4, 4], "data_offsets": [0, 64]}}).encode()
header += b" " * (-len(header) % 8)
open(sys.argv[1], "wb").write(struct.pack("<Q", len(header)) + header + os.urandom(64))
PY
watched upload-b model upload "$OUT/tiny-b.safetensors" "$model" --lane f32 --source-format as-is --on "$ON" || check "model upload --lane f32 failed"
commit_b=$(jq -r '.. | objects | .commit? // empty' "$OUT/upload-b.ndjson" 2> /dev/null | tail -1) || true
# One route takes any source. A held file, with no path sent:
uv run -q --no-project python -I - "$OUT/tiny-c.safetensors" <<'PY'
import json, os, struct, sys
header = json.dumps({"w": {"dtype": "F16", "shape": [4, 4], "data_offsets": [0, 32]}}).encode()
header += b" " * (-len(header) % 8)
open(sys.argv[1], "wb").write(struct.pack("<Q", len(header)) + header + os.urandom(32))
PY
held=$(sha256sum "$OUT/tiny-c.safetensors" | cut -c1-64) addr=$(jq -r .address ~/.cozy/calcifer/calcifer.lock.holder) token=$(jq -r .token ~/.cozy/calcifer/calcifer.lock.holder)
curl -fsS -o /dev/null -X PUT -H "Authorization: Bearer $token" --data-binary "@$OUT/tiny-c.safetensors" "http://$addr/v1/files/sha256:$held" || check "PUT the held model file failed"
curl -sS -X POST -H "Authorization: Bearer $token" -H "Idempotency-Key: held-$run" \
  -d "$(jq -n --arg s "sha256:$held" --arg d "$model" --arg o "$ON" '{target: "cozy/model_upload", on: $o, input: {source: $s, destination: $d, lane: "f16", source_format: "as-is"}}')" "http://$addr/v1/runs" > "$OUT/upload-held.json"
held_run=$(jq -r '.run.number // empty' "$OUT/upload-held.json")
if [ -n "$held_run" ]; then
  cozy --json run watch "$held_run" > /dev/null 2>&1 || true
  cozy --json run watch "$held_run" 2> /dev/null | J '.status == "completed"' || check "model upload of a held file: run $held_run did not complete"
else check "model upload of a held file: $(head -c 300 "$OUT/upload-held.json")"; fi
# And a Hub commit, recorded with nothing uploaded.
cozy --json model upload "$model@$commit" "$model-copy" --lane f16 > "$OUT/upload-recorded.json" 2> "$OUT/upload-recorded.err" \
  && J --arg f "$model@$commit" '.from == $f and .lane == "f16"' "$OUT/upload-recorded.json" \
  || check "model upload of a Hub commit: $(tail -c 300 "$OUT/upload-recorded.json") $(tail -c 300 "$OUT/upload-recorded.err")"
cozy --json model delete "$model-copy" -y > /dev/null 2>&1 || check "model delete of the recorded copy failed"
# While its commit is private (its random bytes are in no public release): a run capability reaches it and nothing else (cl-675).
# Its own process: `cozy` there is the installed binary, not this suite's logging function.
PATH="$(dirname "$COZY"):$PATH" "$here/../capabilities/run.sh" --out "$OUT/capabilities" --commit "$model@$commit" > "$OUT/capabilities.txt" \
  || check "run capabilities: $(grep FAIL "$OUT/capabilities.txt" | head -3 | tr '\n' ' ')"
# Install the private commit on the fixture's local machine; the capability suite above checks scoped access.
watched download-private model install "$model@$commit" --on "$M" \
  || check "model install $model@$commit --on $M: the machine did not read the private commit: $(run_get "$OUT" download-private '.reason.message' | head -c 300)"
# xs-173: the model has no release, and its owner names it by name: the machine resolves it under the run capability's model
# read (Calcifer signs it, TensorD redeems it, the Hub answers the newest commit of the lane). A Hub before it has no model reads.
if curl -fsS "$COZY_HUB_URL/.well-known/oauth-protected-resource" | J '.authorization_details_types_supported | index("tensorhub_model_read")'; then
  watched download-unversioned model install "$model" --lane f32 --on "$M" \
    && cozy --json model ls --on "$M" | J --arg m "$model" '.models | any(.model == $m and .lane == "f32" and .release == "" and .complete)' \
    || check "xs-173: model install $model --lane f32 (no release) by its owner: $(run_get "$OUT" download-unversioned '.reason.message' | head -c 300)"
  watched download-unversioned-any model install "$model" --on "$M" \
    || check "xs-173: model install $model (no release, no lane) by its owner: $(run_get "$OUT" download-unversioned-any '.reason.message' | head -c 300)"
fi
cozy --json model edit "$model" --family e2e-family --tag 'E2E Tag' --tag nsfw --tag anime --untag nsfw > /dev/null \
  && model_details "$model" | J '.family == "e2e-family" and .family_from == "owner" and .tags == ["anime", "e2e-tag"]' \
  || check "model edit: family or tags not set"
# --untag is read as --tag is: any case, spaces around it. One the model doesn't have is removed already: no refusal, nothing else changed.
cozy --json model edit "$model" --untag ' ANIME ' > /dev/null && model_details "$model" | J '.tags == ["e2e-tag"]' \
  || check "model edit --untag ' ANIME ': $(model_details "$model" | jq -c .tags)"
cozy --json model edit "$model" --untag 'E2E Tag' --tag anime > /dev/null && model_details "$model" | J '.tags == ["anime"]' \
  || check "model edit --untag 'E2E Tag' --tag anime: $(model_details "$model" | jq -c .tags)"
cozy --json model edit "$model" --untag never-there > "$OUT/untag-absent.json" 2>&1 && model_details "$model" | J '.tags == ["anime"] and .family == "e2e-family"' \
  || check "model edit --untag never-there: $(tail -c 300 "$OUT/untag-absent.json") $(model_details "$model" | jq -c '{tags, family, trigger}')"
cozy --json model edit "$model" --tag e2e-tag > /dev/null || check "model edit --tag e2e-tag failed"
cozy --json model edit "$model" --family none --untag e2e-tag --untag anime > /dev/null \
  && model_details "$model" | J '.family == null and (.tags // []) == []' \
  || check "model edit --family none --untag e2e-tag --untag anime: not cleared"
# Adapter metadata belongs to the LoRA command group. No inference is needed for this fixture.
uv run -q --no-project python -I - "$OUT/tiny-lora.safetensors" <<'PYLORA'
import json, struct, sys
header = json.dumps({"layer.lora_A.weight": {"dtype": "F16", "shape": [2, 4], "data_offsets": [0, 16]}, "layer.lora_B.weight": {"dtype": "F16", "shape": [4, 2], "data_offsets": [16, 32]}}).encode()
header += b" " * (-len(header) % 8)
open(sys.argv[1], "wb").write(struct.pack("<Q", len(header)) + header + bytes(32))
PYLORA
watched upload-lora lora upload "$OUT/tiny-lora.safetensors" "$model-lora" --lane f16 --source-format as-is --on "$ON" || check "LoRA upload failed"
cozy --json lora edit "$model-lora" --family e2e-family --tag e2e-lora --trigger e2e --strength 0.8 > /dev/null \
  && model_details "$model-lora" | J '.family == "e2e-family" and .tags == ["e2e-lora"] and .trigger == "e2e" and .strength == 0.8' \
  || check "LoRA metadata was not set"
cozy --json lora edit "$model-lora" --trigger '' --strength none > /dev/null \
  && model_details "$model-lora" | J '.trigger == null and .strength == null' || check "LoRA metadata was not cleared"
# xs-173: its owner finds the unreleased LoRA, marked private: by its exact name under either verb, and by search text `org/`
# at a Hub that matches `org/name` (the one with model reads).
if curl -fsS "$COZY_HUB_URL/.well-known/oauth-protected-resource" | J '.authorization_details_types_supported | index("tensorhub_model_read")'; then
  cozy --json lora ls "$WHO/" > "$OUT/search-owner.json" && J --arg m "$model-lora" '.models | any(.model == $m and .private)' "$OUT/search-owner.json" \
    || check "xs-173: lora ls $WHO/ does not list the owner's unreleased $model-lora as private: $(head -c 300 "$OUT/search-owner.json")"
fi
cozy --json model ls "$model-lora" > "$OUT/search-exact.json" && J --arg m "$model-lora" '.models | any(.model == $m and .private)' "$OUT/search-exact.json" \
  || check "xs-173: model ls $model-lora (exact) does not show the owner's unreleased LoRA as private: $(head -c 300 "$OUT/search-exact.json")"
lora_checkpoint=$(jq -r '.. | objects | .commit? // empty' "$OUT/upload-lora.ndjson" | tail -1)
cozy --json lora publish "$model-lora" 0.1.0 "$lora_checkpoint" > "$OUT/lora-publish.json" || check "LoRA publish failed"
cozy --json lora unpublish "$model-lora@0.1.0" > "$OUT/lora-unpublish.json" || check "LoRA unpublish failed"
model_details "$model-lora" | J '.releases | any(.version == "0.1.0" and .visibility == "private")' || check "LoRA unpublish did not retain its private version"
if cozy --json lora delete "$model-lora@$lora_checkpoint" -y > "$OUT/referenced-lora-delete.json" 2>&1; then
  check "a retained private release did not protect its checkpoint from deletion"
fi
cozy --json lora delete "$model-lora" -y > /dev/null || check "LoRA fixture cleanup failed"
cozy --json model publish "$model" 0.1.0 "$commit" "$commit_b" > "$OUT/model-publish.json" || check "model publish failed"
model_details "$model" > "$OUT/model-info.json" \
  && J '.releases | any(.version == "0.1.0" and ([.lanes[].lane] | sort) == ["f16", "f32"])' "$OUT/model-info.json" \
  || check "model publish: no 0.1.0 with lanes f16 and f32"
# Ordinary owners can withdraw and keep using an explicit private version; publication
# stays gated by live admin permission. All account changes are in this disposable Hub.
model_details "$model" > "$OUT/model-before-unpublish.json" || check "could not read the model before unpublish"
"${hub[@]}" admin revoke "$WHO" > "$OUT/owner-admin-revoke.log" 2>&1 || check "could not revoke the fixture publisher's admin role"
cozy --json model unpublish "$model@0.1.0" > "$OUT/model-unpublish.json" || check "owner model unpublish failed"
model_details "$model" > "$OUT/private-model.json" \
  && J --slurpfile before "$OUT/model-before-unpublish.json" \
    '(.checkpoints | map({checkpoint,lane,bytes}) | sort_by(.checkpoint)) == ($before[0].checkpoints | map({checkpoint,lane,bytes}) | sort_by(.checkpoint))' "$OUT/private-model.json" \
  && J '.releases | any(.version == "0.1.0" and .visibility == "private")' "$OUT/private-model.json" \
  || check "unpublish changed the model's checkpoints or lost its private version"
status=$(curl -sS -o "$OUT/private-public-card.json" -w '%{http_code}' "$COZY_HUB_URL/v1/models/$model")
[ "$status" = 404 ] || check "unpublished model remained publicly visible (HTTP $status)"
watched install-private-version model install "$model@0.1.0" --lane f16 --on "$ON" \
  || check "owner private-version install failed: $(run_get "$OUT" install-private-version '.reason.message')"
cozy --json model unpublish "$model@0.1.0" > "$OUT/model-unpublish-again.json" \
  && J '.unpublished == false' "$OUT/model-unpublish-again.json" || check "unpublish was not idempotent"
"${hub[@]}" admin grant "$WHO" > "$OUT/owner-admin-grant.log" 2>&1 || check "could not restore the isolated fixture publisher"
cozy --json model publish "$model" 0.1.0 "$commit" "$commit_b" > "$OUT/model-republish.json" || check "identical release republish failed"
model_details "$model" | J '.releases | any(.version == "0.1.0" and .visibility == "public")' || check "republish did not restore public availability"
cozy --json model delete "$model" -y > "$OUT/model-delete.json" || check "model delete failed"
! model_details "$model" > /dev/null 2>&1 || check "model delete: $model still answers"


step "the content store on $M: a held output across a TensorD restart, a cut upload resumed"
# cl-843: run objects, held outputs and uploads are the machine's TensorFS store's. A weights output stays on the machine;
# after TensorD restarts, the live hold rebuilt from its journal keeps it, and an upload of it reads it there.
ln -sfn "$here/../../release-gate/suites/tensorfs/multi-conversion" "$OUT/ci-multi-conversion"
src=''
if (cd "$OUT" && run_cell "$OUT" held-source ./ci-multi-conversion/source seed:=$RANDOM --on "$M"); then src=$(cat "$OUT/held-source.number")
else check "held-source: a weights output on $M did not complete: $(run_get "$OUT" held-source '.reason.message' | head -c 300)"; fi
was=$(cozy --json machine show "$M" | jq -r '.status.started_at_ms // empty')
cozy --json machine update "$M" --gpu-memory-limit off -y > "$OUT/restart.json" 2>&1 \
  || check "machine update $M --gpu-memory-limit off, a restart: $(tail -c 300 "$OUT/restart.json")"
now=$(cozy --json machine show "$M" | jq -r '.status.started_at_ms // empty')
[ -n "$now" ] && [ "$now" != "$was" ] || check "machine update $M --gpu-memory-limit off: TensorD did not restart (started at $was, then $now)"
if [ -n "$src" ]; then
  watched held-upload model upload "run:$src#model" "$model-store" --lane bf16 --on "$M" \
    || check "held-upload: run $src's output after TensorD restarted: $(run_get "$OUT" held-upload '.reason.message' | head -c 300)"
fi
# A cut upload resumes: release-gate/suites/tensord/pod-cells.sh (pc-resume) cuts one by killing Calcifer. A cancel never cuts
# one: the canceled run's upload goes on and the next run of the file waits for it (kept so: dedup, 11-09).

step "a model upload by $M over an 8 Mbit/s link to the store (xs-172)"
. "$here/../../release-gate/suites/hub/slow-upload.sh"
hub_slow_upload


)
release_synthetic_checks
