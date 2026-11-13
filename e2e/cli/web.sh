# e2e/cli/web.sh: the page in a browser. The web suite (cl-973): web/<cell>.spec.js, a lane of them at once on this job's Calcifer
# (web_suite). What of the page needs no browser is a CLI part: commands.sh's page_http. By hand, `--shard web-<cell>` also runs a
# web/<cell>.js outside the suite (web_alone): look (pictures to review), scale (cl-885's benchmark), and the cells not yet in the suite.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
J() { jq -e "$@" > /dev/null; }

# A path as a program outside this shell reads it.
web_path() { if [ "${RUNNER_OS:-}" = Windows ]; then cygpath -m "$1"; else echo "$1"; fi; }
# Each runner image has Google Chrome where its OS puts it.
web_chrome() {
  case ${RUNNER_OS:-$(uname -s)} in
    Windows) echo 'C:/Program Files/Google/Chrome/Application/chrome.exe' ;;
    macOS | Darwin) echo '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' ;;
    *) command -v google-chrome || true ;;
  esac
}

# web_suite <lane> <cell...> (cl-973): each cell web/<cell>.spec.js, several at once through Playwright Test (web/playwright.config.js)
# on this job's Calcifer, each on a project and a browser context of its own. Then one check a failed cell, and each cell's time and the
# 1-minute load as it ended. <lane>: 1, 2, 3 (Chrome), engines (Firefox's `@engines` cells, and `@vault`), solo (one at a time), native, or a
# cell's own name (by hand: whichever project takes it).
web_suite() {
  local lane=$1 pw=$OUT/playwright-test chrome cell engine code line web=$OUT/web
  local projects=() workers=() filters=()
  shift
  [ $# -gt 0 ] || { echo "web-$lane: no cells written yet"; return 0; }
  step "web-$lane: $*"
  chrome=$(web_chrome)
  [ -n "$chrome" ] && [ -x "$chrome" ] || { check "web-$lane: no Google Chrome on this runner (${chrome:-none})"; return; }
  for cell in "$@"; do
    [ -f "$here/web/$cell.spec.js" ] || { check "web-$lane: no web/$cell.spec.js"; return; }
    # A file pattern is a regex on its path: anchored, `create` can't take every file under cozy-creator/.
    filters+=("[\\/]$cell\\.spec\\.js\$")
  done
  npm install -s --prefix "$(web_path "$pw")" @playwright/test@1.55.0 > "$OUT/web-npm.log" 2>&1 \
    || { check "web-$lane: npm install @playwright/test failed: $(tail -c 300 "$OUT/web-npm.log")"; return; }
  case $lane in
    1 | 2 | 3) projects=(chrome) ;;
    engines)
      projects=(firefox vault)
      # H.264 is the system's libavcodec on Linux: with none, Firefox says "This browser can't play it" (41966978063).
      bash "$here/install-browser-deps.sh" "$pw" > "$OUT/web-firefox-deps.log" 2>&1 \
        || { check "web-$lane: Firefox's dependencies failed: $(tail -c 300 "$OUT/web-firefox-deps.log")"; return; } ;;
    solo) projects=(solo) workers=(--workers=1) ;;
    native) projects=(chrome solo); [ "${RUNNER_OS:-}" != macOS ] || projects+=(webkit) ;;
    *) projects=(chrome vault solo) ;;
  esac
  for engine in firefox webkit; do
    [[ " ${projects[*]} " != *" $engine "* ]] || node "$pw/node_modules/playwright-core/cli.js" install "$engine" > "$OUT/web-$engine-install.log" 2>&1 \
      || { check "web-$lane: installing $engine failed: $(tail -c 300 "$OUT/web-$engine-install.log")"; return; }
  done
  rm -rf "$web/results" "$web/report.json"
  mkdir -p "$web"
  {
    HOLDER=$(web_path "$HOME/.cozy/calcifer/calcifer.lock.holder") COZY=$(web_path "$COZY") CHROME=$chrome OUT=$(web_path "$OUT") ARGV_LOG=$(web_path "$OUT/argv.log") \
      NODE_PATH=$(web_path "$pw/node_modules") node "$(web_path "$pw/node_modules/@playwright/test/cli.js")" test -c "$(web_path "$here/web/playwright.config.js")" \
      "${projects[@]/#/--project=}" ${workers[@]+"${workers[@]}"} "${filters[@]}" 2>&1 && echo 0 > "$web/$lane.code" || echo $? > "$web/$lane.code"
  } | tee "$web/$lane.log"
  code=$(cat "$web/$lane.code")
  # A trace is a zip, which e2e/publish's scrub can't read: unpacked, its bearers and codes are scrubbed like any text.
  find "$web/results" -name trace.zip 2> /dev/null | while read -r trace; do
    python3 -m zipfile -e "$trace" "${trace%.zip}" && rm -f "$trace"
  done || true
  [ -s "$web/report.json" ] || { check "web-$lane: Playwright wrote no report (exit $code): $(tail -c 600 "$web/$lane.log")"; return; }
  node "$(web_path "$here/web/report.js")" "$(web_path "$web/report.json")" "$@" > "$web/$lane.said" 2>&1 \
    || { check "web-$lane: its report doesn't read: $(tail -c 600 "$web/$lane.said")"; return; }
  local failed=0
  while IFS= read -r line; do
    case $line in
      FAILED\ *) check "${line#FAILED }"; failed=1 ;;
      *) echo "$line" ;;
    esac
  done < "$web/$lane.said"
  [ "$code" = 0 ] || [ "$failed" = 1 ] || check "web-$lane: Playwright exited $code with no cell failed: $(tail -c 600 "$web/$lane.log")"
  echo "web-$lane: load now $(cut -d' ' -f1-3 /proc/loadavg 2> /dev/null)"
}

# web_alone [cell]: web/<cell>.js ($WEB_ALONE_CELL), by hand, signed in by its own `cozy open` (page.js: signIn) on a project of its own,
# whose folder it deletes whatever happened. $RESTART kills this Calcifer and brings it back, for a cell that asks for it (stays).
web_alone() {
  local name=${1:-$WEB_ALONE_CELL} pw=$OUT/playwright chrome json code=0 folder lock=$HOME/.cozy/calcifer/calcifer.lock.holder restart
  step "web-$name, by hand"
  chrome=$(web_chrome)
  [ -n "$chrome" ] && [ -x "$chrome" ] || { check "web-$name: no Google Chrome on this runner (${chrome:-none})"; return; }
  npm install -s --prefix "$(web_path "$pw")" playwright-core@1.55.0 > "$OUT/web-npm.log" 2>&1 \
    || { check "web-$name: npm install playwright-core failed: $(tail -c 300 "$OUT/web-npm.log")"; return; }
  if [ "${RUNNER_OS:-}" = Windows ]; then restart="taskkill //F //PID \$(jq -r .pid < '$lock') > /dev/null"; else restart="kill -9 \$(jq -r .pid < '$lock')"; fi
  restart="$restart; sleep 1; '$COZY' machine ls > /dev/null 2>&1"
  json=$OUT/web-$name.json
  HOLDER=$(web_path "$lock") ARGV_LOG=$(web_path "$OUT/argv.log") RESTART=$restart LAST_LOOK=$(web_path "$OUT/web-$name") SHOT=$(web_path "$OUT/web-$name") \
    CHROME=$chrome PLAYWRIGHT=$(web_path "$pw/node_modules/playwright-core") COZY=$(web_path "$COZY") \
    node "$(web_path "$here/web/$name.js")" > "$json" 2> "$OUT/web-$name.err" || code=$?
  [ "$code" = 0 ] || check "web-$name: failed $(jq -c .failures "$json" 2> /dev/null): $(jq -c '(.failures // []) as $f | .seen | with_entries(select(.key as $k | $f | index($k)))' "$json" 2> /dev/null | head -c 2000) $(tail -c 500 "$OUT/web-$name.err")"
  folder=$(jq -r '.seen.project.folder // empty' "$json" 2> /dev/null)
  case $folder in */web-*) rm -rf "$folder" 2> /dev/null || { sleep 5; rm -rf "$folder"; } || true ;; esac
}
