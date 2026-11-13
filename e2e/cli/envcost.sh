# cl-984: what a second project's first install of a package costs against the first's. Since #2858 each project builds its
# environments in its own engine (<project>/.vault-machine). Two unencrypted projects each install qwen-image-2, then
# minimax-h3, from the packages checkout (`cozy package install <dir> --on local`: local code installs into its project);
# project a installs both again (held); then both by name from the Hub, from inside project a. Installs only, no model runs.
# Each step's wall time, its run's stages, and allocated bytes by area (envcost.py) land in envcost.jsonl and envcost.md.
# Then BiRefNet's weights in each (weights_cost). By dispatch.
env_cost() {
[ -n "$LOCAL" ] || return 0
step "env-cost (cl-984): a second project's first install of qwen-image-2 and minimax-h3 against the first's"
[ -n "$PACKAGES" ] || { check "env-cost: no packages checkout (--packages)"; return; }
local root=${RUNNER_TEMP:-$HOME}/envcost p
ENVCOST_ROOT=$root ENVCOST_ROWS=$OUT/envcost.jsonl
ENVCOST_ROOTS=("$root/a/.vault-machine" "$root/b/.vault-machine" "$HOME/.cozy" "${TENSORFS_HOME:-$HOME/.cache/tensorfs}")
rm -rf "$root" && mkdir -p "$root" && : > "$ENVCOST_ROWS"
df -B1 / "$root" > "$OUT/envcost-df-start.txt" 2>&1 || true
cat "$OUT/envcost-df-start.txt"
for p in a b; do
  cozy --json new --unencrypted "$root/$p" > "$OUT/envcost-new-$p.json" 2>&1 \
    || { check "env-cost: cozy new --unencrypted $p: $(tail -c 300 "$OUT/envcost-new-$p.json")"; return; }
done
envcost_install "project a: qwen-image-2" a "$PACKAGES/qwen-image-2"
envcost_install "project a: minimax-h3" a "$PACKAGES/minimax-h3"
envcost_install "project b: qwen-image-2" b "$PACKAGES/qwen-image-2"
envcost_install "project b: minimax-h3" b "$PACKAGES/minimax-h3"
envcost_install "project a: qwen-image-2 again" a "$PACKAGES/qwen-image-2"
envcost_install "project a: minimax-h3 again" a "$PACKAGES/minimax-h3"
envcost_install "Hub qwen-image-2, from project a" home qwen-image-2
envcost_install "Hub minimax-h3, from project a" home minimax-h3
weights_cost
df -B1 / "$root" > "$OUT/envcost-df-end.txt" 2>&1 || true
python3 "$here/envcost.py" table "$ENVCOST_ROWS" | tee "$OUT/envcost.md" || check "env-cost: no table"
[ -z "${GITHUB_STEP_SUMMARY:-}" ] || { echo "### env-cost (cl-984)"; cat "$OUT/envcost.md"; } >> "$GITHUB_STEP_SUMMARY"
}

# The weights case: BiRefNet (the isolated Hub's mirror of ZhengPeng7/BiRefNet, isolated.sh) installed in each project, then
# run on the CPU in project a, project b, and project a again on another picture. Each run's downloaded bytes (its
# `downloading` progress), the loopback bytes received while it ran (the isolated Hub and its MinIO are on this runner), each
# store's growth, its preparation and execution from the run record, and its stages (the executor's start among them).
weights_cost() {
step "env-cost (cl-984): BiRefNet's weights in a second project"
envcost_install "project a: birefnet" a "$PACKAGES/birefnet"
envcost_install "project b: birefnet" b "$PACKAGES/birefnet"
weights_run "project a: BiRefNet first run" a "$(picture dog1.jpg)"
weights_run "project b: BiRefNet first run" b "$(picture dog1.jpg)"
weights_run "project a: BiRefNet again" a "$(picture dog2.jpg)"
# One machine's executors serve every project (cl-984): b's next run takes the executor a's left warm, then both at once.
weights_run "project b: BiRefNet on a's warm executor" b "$(picture dog2.jpg)"
local pair=$OUT/envcost-pair astronaut
mkdir -p "$pair"
astronaut=$(picture astronaut.jpg)
for p in a b; do
  (cd "$ENVCOST_ROOT/$p" && timeout 900 "$COZY" --json run "$(birefnet)" --file "image=$astronaut" --on local > "$pair/$p.ndjson" 2> "$pair/$p.err"; echo $? > "$pair/$p.code") &
done
wait
for p in a b; do
  [ "$(cat "$pair/$p.code")" = 0 ] || check "env-cost: project $p's concurrent BiRefNet run exited $(cat "$pair/$p.code"): $(tail -c 300 "$pair/$p.err")"
  echo "concurrent $p: $(grep '^{' "$pair/$p.ndjson" | jq -r 'select(.type == "run.progress") | .progress.stage' | uniq | paste -sd';' - | cut -c1-400)"
done
# TensorFS's transport logs (a line per completed fetch), wherever a store keeps one.
local f
while IFS= read -r f; do
  cp "$f" "$OUT/envcost-transport-$(printf '%s' "${f#"$ENVCOST_ROOT"/}" | tr '/' '_')" || true
done < <(find "${ENVCOST_ROOTS[@]}" -name 'transport.log*' -type f 2> /dev/null)
}

# weights_run <label> <a|b> <picture>: `cozy --json run <birefnet> --file image=<picture> --on local` in project <a|b>, measured.
weights_run() {
local label=$1 at=$2 pic=$3 name t0 t1 rx0 rx1 code=0 dir=$ENVCOST_ROOT/$2 n
name=$(printf '%s' "$label" | tr 'A-Z' 'a-z' | tr -c 'a-z0-9' '-')
python3 "$here/envcost.py" measure "${ENVCOST_ROOTS[@]}" > "$OUT/envcost-$name-before.json" || true
rx0=$(awk '$1 == "lo:" { print $2 }' /proc/net/dev)
t0=$(date +%s.%N)
(cd "$dir" && timeout 900 "$COZY" --json run "$(birefnet)" --file "image=$pic" --on local) > "$OUT/envcost-$name.ndjson" 2> "$OUT/envcost-$name.err" || code=$?
t1=$(date +%s.%N)
rx1=$(awk '$1 == "lo:" { print $2 }' /proc/net/dev)
n=$(grep '^{' "$OUT/envcost-$name.ndjson" | jq -r 'select((.run.number? // 0) > 0) | .run.number' 2> /dev/null | tail -1 || true)
[ -z "$n" ] || (cd "$dir" && timeout 60 "$COZY" --json run watch "$n" --once) > "$OUT/envcost-$name.record.json" 2>&1 || true
python3 "$here/envcost.py" measure "${ENVCOST_ROOTS[@]}" > "$OUT/envcost-$name-after.json" || true
jq -n --argjson rx "$((rx1 - rx0))" --arg home "$HOME/.cozy" --arg tfs "${ENVCOST_ROOTS[3]}" \
  '{kind: "run", loopback: $rx, home: $home, tensorfs: $tfs}' > "$OUT/envcost-$name.extra.json"
python3 "$here/envcost.py" row "$label" "$dir/.vault-machine" "$OUT/envcost-$name-before.json" "$OUT/envcost-$name-after.json" \
  "$OUT/envcost-$name.ndjson" "$(echo "$t1 - $t0" | bc)" "$code" "$OUT/envcost-$name.extra.json" >> "$ENVCOST_ROWS" || check "env-cost: $label: no row"
echo "$label: run ${n:-none} exit $code in $(echo "$t1 - $t0" | bc) s, loopback $((rx1 - rx0)) B"
[ "$code" = 0 ] || check "env-cost: $label: cozy run $(birefnet) exited $code: $(tail -c 300 "$OUT/envcost-$name.err")"
}

# envcost_install <label> <a|b|home> <target>: `cozy --json package install <target> --on local` from project a (home) or <a|b>,
# timed and measured before and after.
envcost_install() {
local label=$1 at=$2 target=$3 name t0 t1 code=0 dir project
name=$(printf '%s' "$label" | tr 'A-Z' 'a-z' | tr -c 'a-z0-9' '-')
dir=$ENVCOST_ROOT/$at project=$ENVCOST_ROOT/$at/.vault-machine
[ "$at" != home ] || { dir=$ENVCOST_ROOT/a project=$HOME/.cozy; }
local roots=("${ENVCOST_ROOTS[@]}")
python3 "$here/envcost.py" measure "${roots[@]}" > "$OUT/envcost-$name-before.json" || true
t0=$(date +%s.%N)
# Bounded: a hang is this step's failure, never the shard's.
(cd "$dir" && timeout 1200 "$COZY" --json package install "$target" --on local) > "$OUT/envcost-$name.ndjson" 2> "$OUT/envcost-$name.err" || code=$?
t1=$(date +%s.%N)
python3 "$here/envcost.py" measure "${roots[@]}" > "$OUT/envcost-$name-after.json" || true
python3 "$here/envcost.py" row "$label" "$project" "$OUT/envcost-$name-before.json" "$OUT/envcost-$name-after.json" \
  "$OUT/envcost-$name.ndjson" "$(echo "$t1 - $t0" | bc)" "$code" >> "$ENVCOST_ROWS" || check "env-cost: $label: no row"
echo "$label: exit $code in $(echo "$t1 - $t0" | bc) s; $(df -h --output=used,avail "$ENVCOST_ROOT" | tail -1)"
[ "$code" = 0 ] || check "env-cost: $label: cozy package install $target exited $code: $(tail -c 300 "$OUT/envcost-$name.err")"
}
