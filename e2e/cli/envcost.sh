# cl-984: what a second project's first install of a package costs against the first's. Since #2858 each project builds its
# environments in its own engine (<project>/.vault-machine). Two unencrypted projects each install qwen-image-2, then
# minimax-h3, from the packages checkout (`cozy package install <dir> --on local`: local code installs into its project);
# project a installs both again (held); then both by name from the Hub, from inside project a. Install only: no model runs.
# Each step's wall time, its run's stages, and allocated bytes by area (envcost.py) land in envcost.jsonl and envcost.md.
# By dispatch.
env_cost() {
[ -n "$LOCAL" ] || return 0
step "env-cost (cl-984): a second project's first install of qwen-image-2 and minimax-h3 against the first's"
[ -n "$PACKAGES" ] || { check "env-cost: no packages checkout (--packages)"; return; }
local root=${RUNNER_TEMP:-$HOME}/envcost p
ENVCOST_ROOT=$root ENVCOST_ROWS=$OUT/envcost.jsonl
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
df -B1 / "$root" > "$OUT/envcost-df-end.txt" 2>&1 || true
python3 "$here/envcost.py" table "$ENVCOST_ROWS" | tee "$OUT/envcost.md" || check "env-cost: no table"
[ -z "${GITHUB_STEP_SUMMARY:-}" ] || { echo "### env-cost (cl-984)"; cat "$OUT/envcost.md"; } >> "$GITHUB_STEP_SUMMARY"
}

# envcost_install <label> <a|b|home> <target>: `cozy --json package install <target> --on local` from project a (home) or <a|b>,
# timed and measured before and after.
envcost_install() {
local label=$1 at=$2 target=$3 name t0 t1 code=0 dir project
name=$(printf '%s' "$label" | tr 'A-Z' 'a-z' | tr -c 'a-z0-9' '-')
dir=$ENVCOST_ROOT/$at project=$ENVCOST_ROOT/$at/.vault-machine
[ "$at" != home ] || { dir=$ENVCOST_ROOT/a project=$HOME/.cozy; }
local roots=("$ENVCOST_ROOT/a/.vault-machine" "$ENVCOST_ROOT/b/.vault-machine" "$HOME/.cozy")
python3 "$here/envcost.py" measure "${roots[@]}" > "$OUT/envcost-$name-before.json" || true
t0=$(date +%s.%N)
in_project "$dir" --json package install "$target" --on local > "$OUT/envcost-$name.ndjson" 2> "$OUT/envcost-$name.err" || code=$?
t1=$(date +%s.%N)
python3 "$here/envcost.py" measure "${roots[@]}" > "$OUT/envcost-$name-after.json" || true
python3 "$here/envcost.py" row "$label" "$project" "$OUT/envcost-$name-before.json" "$OUT/envcost-$name-after.json" \
  "$OUT/envcost-$name.ndjson" "$(echo "$t1 - $t0" | bc)" "$code" >> "$ENVCOST_ROWS" || check "env-cost: $label: no row"
echo "$label: exit $code in $(echo "$t1 - $t0" | bc) s; $(df -h --output=used,avail "$ENVCOST_ROOT" | tail -1)"
[ "$code" = 0 ] || check "env-cost: $label: cozy package install $target exited $code: $(tail -c 300 "$OUT/envcost-$name.err")"
}
