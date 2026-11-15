# xs-189: what an idle cozy costs. Calcifer and TensorD with nothing to do, again with an idle executor after one
# weightless run, again after a model's download, and again with the page open as Paul keeps it, on an unlocked encrypted
# project and a plain one: each cozy process's CPU time and wakeups over 60 s, after 30 s to settle, and its threads by name
# (idle.py; idle.json, idle-warm.json, idle-downloaded.json, idle-page.json). By dispatch: a PR that changes a loop puts its
# before and after in its Proof.
idle_cost() {
[ "$RUNNER_OS" = Linux ] || return 0
step "an idle Calcifer and TensorD: CPU time and wakeups over 60 s (xs-189)"
python3 "$here/idle.py" 30 60 "$OUT/idle.json" 2> "$OUT/idle.err" || check "idle: $(tail -c 300 "$OUT/idle.err")"
step "the same with an idle executor (xs-189)"
cell idle-warm "$here/weightless/reverse" "text=idle$RANDOM" || return 0
python3 "$here/idle.py" 30 60 "$OUT/idle-warm.json" 2> "$OUT/idle-warm.err" || check "idle-warm: $(tail -c 300 "$OUT/idle-warm.err")"
step "the same after a model's download (xs-189)"
cozy --json model install lilylilith/fontbox --on local > "$OUT/idle-download.ndjson" 2>&1 \
  || { check "idle-downloaded: model install failed: $(tail -c 300 "$OUT/idle-download.ndjson")"; return 0; }
python3 "$here/idle.py" 30 60 "$OUT/idle-downloaded.json" 2> "$OUT/idle-downloaded.err" || check "idle-downloaded: $(tail -c 300 "$OUT/idle-downloaded.err")"
step "the same with the page open on an unlocked encrypted project and a plain one (xs-189)"
idle_page
}

idle_page() {
local dir=$OUT/idle-page pw=$OUT/playwright chrome page
chrome=$(web_chrome)
[ -n "$chrome" ] && [ -x "$chrome" ] || { check "idle-page: no Google Chrome on this runner (${chrome:-none})"; return; }
npm install -s --prefix "$pw" playwright-core@1.55.0 > "$OUT/idle-npm.log" 2>&1 || { check "idle-page: npm install playwright-core failed: $(tail -c 300 "$OUT/idle-npm.log")"; return; }
# The project-encryption driver, as the projectvault shard's job installs it.
[ "${GITHUB_ACTIONS:-}" != true ] || bash "$here/install-vault-driver.sh" > "$OUT/idle-driver.log" 2>&1 \
  || { check "idle-page: the encryption driver didn't install: $(tail -c 300 "$OUT/idle-driver.log")"; return; }
mkdir -p "$dir" && printf '%s\n' 'synthetic idle vault passphrase' > "$dir/passphrase" && chmod 600 "$dir/passphrase"
cozy --json new "$dir/encrypted" --passphrase-file "$dir/passphrase" --recovery-out "$dir/recovery" > "$dir/encrypted.json" 2>&1 \
  || { check "idle-page: new encrypted project: $(tail -c 300 "$dir/encrypted.json")"; return; }
cozy --json new --unencrypted "$dir/plain" > "$dir/plain.json" 2>&1 || { check "idle-page: new plain project: $(tail -c 300 "$dir/plain.json")"; return; }
HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder CHROME=$chrome PLAYWRIGHT=$pw/node_modules/playwright-core \
  node "$here/web/idle.js" "$dir/encrypted" "$dir/plain" > "$dir/page.out" 2> "$dir/page.err" &
page=$!
for _ in $(seq 120); do grep -qx open "$dir/page.out" && break; kill -0 "$page" 2> /dev/null || break; sleep 1; done
if grep -qx open "$dir/page.out"; then
  python3 "$here/idle.py" 30 60 "$OUT/idle-page.json" 2> "$OUT/idle-page.err" || check "idle-page: $(tail -c 300 "$OUT/idle-page.err")"
else
  check "idle-page: the page didn't open both projects: $(tail -c 300 "$dir/page.err")"
fi
kill "$page" 2> /dev/null; wait "$page" 2> /dev/null
in_project "$dir/encrypted" --json vault lock > "$dir/lock.json" 2>&1 || check "idle-page: the encrypted project doesn't lock: $(tail -c 300 "$dir/lock.json")"
}
