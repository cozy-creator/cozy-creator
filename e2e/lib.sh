# e2e/lib.sh: what the e2e suites share. A suite drives `cozy` the way a person does, in the default ~/.cozy,
# on a fresh GitHub runner: the real installer, real packages and models from tensorhub.com, real outputs.
#
#   source e2e/lib.sh; parse "$@"
#   --out DIR             where records, outputs and logs go
#   --tensord FILE|V      a candidate `cozy` (a dev build, or an exact release version): it becomes this computer's
#                         cozy and its machine
#   --tensorfs FILE|V     a candidate TensorFS for the local machine (a wheel, or an exact release version)
#   --runtime FILE|V      a candidate Runtime for the local machine (a wheel, or an exact release version)
#   --packages DIR        a packages checkout: its package directories run instead of the published releases
#   --pins N@R,...        package releases to take instead of the latest unyanked ones (gated candidates)
#   --published           the imports suite loads published releases, not the packages checkout
#   --key FILE            sign in with this device key (`cozy login --key`; release suite)
#   --sku SKU             what the release suite rents (default: the catalog's cheapest GPU)
#   --max-hourly USD      the release suite rents nothing dearer an hour (default 0.50)
#   --max-usd USD         the release suite stops, ending its rental, once its GPU time would cost more (default 0.07)
#   --quote               the release suite stops at the rental's quote: nothing is rented ($0 pre-flight)
#   --installed           this computer's cozy as it is, signed in: no installer (the release suite by hand)
#   --release DIR         a cozy release's files (cozy-publish.yaml's artifacts, a dry run's too): its own installer
#                         installs cozy from DIR, served on loopback, in place of tensorhub.com's (the candidate)
#   --shard S             one part of the suite, on its own runner (cli: a name; imports: K/N, every Nth package)

set -euo pipefail
E2E=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
OUT='' CAND_COZY='' CAND_TFS='' CAND_RT='' PACKAGES='' PINS='' PUBLISHED='' KEY='' SKU='' INSTALLED='' SHARD='' RELEASE='' MAX_HOURLY=0.50 MAX_USD=0.07 QUOTE=''
COZY=$HOME/.cozy/bin/cozy
[ "${RUNNER_OS:-}" != Windows ] || COZY=$HOME/.cozy/bin/cozy.exe
# Whether this OS runs models on its own machine, `local` (platform::Os::local_inference): Linux. macOS and Windows
# run on rented Linux GPUs, and every verb that would use `local` refuses `local_unavailable`.
LOCAL=1
[ "${RUNNER_OS:-Linux}" = Linux ] || LOCAL=''

parse() {
  while [ $# -gt 0 ]; do
    case $1 in
      --out) OUT=$2 ;;
      --tensord) CAND_COZY=$2 ;;
      --tensorfs) CAND_TFS=$2 ;;
      --runtime) CAND_RT=$2 ;;
      --packages) PACKAGES=$2 ;;
      --pins) PINS=$2 ;;
      --published) PUBLISHED=1; shift; continue ;;
      --key) KEY=$2 ;;
      --sku) SKU=$2 ;;
      --max-hourly) MAX_HOURLY=$2 ;;
      --max-usd) MAX_USD=$2 ;;
      --quote) QUOTE=1; shift; continue ;;
      --installed) INSTALLED=1; shift; continue ;;
      --shard) SHARD=$2 ;;
      --release) RELEASE=$(cd "$2" && pwd) ;;
      *) echo "unknown argument $1" >&2; exit 2 ;;
    esac
    shift 2
  done
  [ -n "$OUT" ] || { echo "--out is required" >&2; exit 2; }
  # By hand (--installed), a --tensord file is the CLI as it is: nothing is installed over this computer's cozy.
  [ -z "$INSTALLED" ] || [ ! -f "$CAND_COZY" ] || { COZY=$CAND_COZY; CAND_COZY=''; }
  mkdir -p "$OUT"
}

step() { echo "::endgroup::"; echo "::group::$*"; }
# fail: the suite cannot go on. check: a failure the suite records and goes past, so one run reports
# every broken step; `finish` exits non-zero when any was recorded.
fail() { echo "::error::$*"; echo "$*" >> "$OUT/failures.txt"; exit 1; }
check() { echo "::error::$*"; echo "$*" >> "$OUT/failures.txt"; }
finish() {
  echo "::endgroup::"
  [ -s "$OUT/failures.txt" ] || { echo "$1: passed"; return 0; }
  echo "$1: $(wc -l < "$OUT/failures.txt") failure(s):"; cat "$OUT/failures.txt"; exit 1
}
# Every call is logged for e2e/coverage.py: its words \x1f-separated, each call ended by \x1e.
cozy() { printf '%s\x1f' "$@" >> "$OUT/argv.log"; printf '\x1e' >> "$OUT/argv.log"; "$COZY" "$@"; }
# The coverage guard (e2e/coverage.py check) over this run's calls, for the tiers it ran.
coverage() {
  uv run -q --no-project python -I "$E2E/coverage.py" check "$1" "$E2E/coverage.toml" --tiers "$2" --log "$OUT/argv.log" \
    --source "$E2E/../crates/cli/src" \
    || check "coverage: a command or flag has no test, or its test did not run (above)"
}

# The install line from the README, as a person pastes it. With --release, that release's own installer then installs
# over it while its Calcifer runs (the README's upgrade from 0.11.17 or older), reading the archives from a loopback
# server (COZY_DOWNLOAD_URL, which `cozy update` reads too), and `cozy doctor` restarts Calcifer on it.
install_cozy() {
  local want ps=powershell
  # Windows PowerShell under the runner's x64 shell says X64 on an Arm64 runner; PowerShell 7 says Arm64, as Windows
  # PowerShell does in a person's own terminal on Windows 11.
  [ "${RUNNER_ARCH:-}" != ARM64 ] || ps=pwsh
  if [ "$RUNNER_OS" = Windows ]; then
    $ps -NoProfile -ExecutionPolicy Bypass -c 'irm https://tensorhub.com/install.ps1 | iex' || fail "the installer failed"
    found=$(powershell -NoProfile -c '[Environment]::GetEnvironmentVariable("Path", "User")' | tr ';' '\n' | tr -d '\r' | grep -Fix "$(cygpath -w "$(dirname "$COZY")")" || true)
    [ -n "$found" ] || fail "the user's PATH does not hold $(dirname "$COZY")"
  else
    curl -LsSf --retry 3 --retry-all-errors https://tensorhub.com/install.sh | sh || fail "the installer failed"
    found=$(env -i HOME="$HOME" PATH=/usr/bin:/bin sh -lc 'command -v cozy' || true)
    [ "$found" = "$COZY" ] || fail "a login shell finds '$found', not $COZY"
  fi
  cozy --version
  [ -n "$RELEASE" ] || return 0
  serve_release
  cozy --json machine list > /dev/null || check "the installed cozy's Calcifer did not start"
  want="cozy $(sed -n 's/^APP_VERSION="\(.*\)"$/\1/p' "$RELEASE/cozy-installer.sh")"
  if [ "$RUNNER_OS" = Windows ]; then
    $ps -NoProfile -ExecutionPolicy Bypass -c "& '$(cygpath -w "$RELEASE")\cozy-installer.ps1'" || fail "the release's installer failed over a running cozy"
  else
    sh "$RELEASE/cozy-installer.sh" || fail "the release's installer failed over a running cozy"
  fi
  [ "$(cozy --version)" = "$want" ] || fail "cozy is $(cozy --version) after the release's installer, not $want"
  native
  cozy --json doctor > "$OUT/doctor-release.json" || check "cozy doctor is not healthy after the release's installer"
  grep -q "cozy ${want#cozy }," "$OUT/doctor-release.json" || check "Calcifer is not the release's ${want#cozy } after cozy doctor"
}

# The installed cozy is this runner's own architecture, never one it emulates: on Windows its PE machine field.
native() {
  local pe machine want=8664 arch='x86[-_]64'
  if [ "$RUNNER_OS" != Windows ]; then
    [ "${RUNNER_ARCH:-}" != ARM64 ] || arch='arm64|aarch64'
    file "$COZY" | tee "$OUT/cozy-file.txt" | grep -Eqi "$arch" || check "the installed cozy is not this runner's $RUNNER_ARCH: $(cat "$OUT/cozy-file.txt")"
    return
  fi
  pe=$(od -An -tu4 -j60 -N4 "$COZY" | tr -d ' ')
  machine=$(od -An -tx2 -j$((pe + 4)) -N2 "$COZY" | tr -d ' ')
  [ "${RUNNER_ARCH:-}" != ARM64 ] || want=aa64
  echo "cozy.exe: PE machine $machine"
  [ "$machine" = "$want" ] || check "the installed cozy.exe is PE machine $machine, not this runner's $want ($RUNNER_ARCH)"
}

# --release's files on a loopback port for the suite's life, as COZY_DOWNLOAD_URL.
serve_release() {
  local port=$((20000 + RANDOM % 20000)) py=python3
  command -v python3 > /dev/null || py=python
  (cd "$RELEASE" && exec "$py" -m http.server "$port" --bind 127.0.0.1 > "$OUT/release-server.log" 2>&1) &
  export COZY_DOWNLOAD_URL=http://127.0.0.1:$port
  for _ in $(seq 30); do curl -fsS -o /dev/null "$COZY_DOWNLOAD_URL/cozy-installer.sh" && return 0; sleep 1; done
  fail "the release's files are not served at $COZY_DOWNLOAD_URL"
}

# A user service manager, as a desktop login has: the runner's user lingers, with its bus.
user_services() {
  [ "$RUNNER_OS" = Linux ] || return 0
  sudo loginctl enable-linger "$(id -un)"
  export XDG_RUNTIME_DIR=/run/user/$(id -u)
  until [ -S "$XDG_RUNTIME_DIR/bus" ]; do sleep 1; done
  export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
}

# Candidates go on through `cozy machine update local`, the path a person takes: a dev build's file, or an exact
# release version (a prerelease's too) the machine fetches and verifies. TensorD first, from the installed cozy, so
# the candidate's own CLI puts the wheels on. CAND_RUNTIME is then the Runtime the machine reports (`machine show
# --json`), which must differ from what it ran before.
CAND_RUNTIME=''
candidates() {
  local args=() before want
  before=$(jq -r '.status.runtime // empty' "$OUT/machine.json" 2> /dev/null || true)
  [ -n "$LOCAL" ] || { candidate_cozy; return; }
  [ -n "$CAND_COZY$CAND_TFS$CAND_RT" ] || return 0
  if [ -n "$CAND_COZY" ]; then
    cozy --json machine update local --tensord "$CAND_COZY" -y | tee "$OUT/update-tensord.json" || fail "cozy machine update local refused TensorD $CAND_COZY"
    if [ -f "$CAND_COZY" ]; then want=$("$CAND_COZY" --version); else want="cozy ${CAND_COZY#v}"; fi
    [ "$(cozy --version)" = "$want" ] || fail "cozy is $(cozy --version), not the candidate $want"
    # The installer's Calcifer keeps running an older cozy until restarted (`cozy update` does that for
    # a person): `cozy doctor` finds it and restarts it on the candidate.
    cozy --json doctor > "$OUT/doctor-candidate.json" || check "cozy doctor is not healthy after the candidates"
    grep -q "cozy ${want#cozy }," "$OUT/doctor-candidate.json" || check "Calcifer is not the candidate ${want#cozy }"
  fi
  [ -z "$CAND_TFS" ] || args+=(--tensorfs "$CAND_TFS")
  [ -z "$CAND_RT" ] || args+=(--runtime "$CAND_RT")
  [ ${#args[@]} -eq 0 ] || cozy --json machine update local "${args[@]}" -y | tee "$OUT/update.json" || fail "cozy machine update local refused the candidates"
  machine_ready
  [ -n "$CAND_RT" ] || return 0
  CAND_RUNTIME=$(jq -r '.status.runtime // empty' "$OUT/machine.json" 2> /dev/null || true)
  if [ -z "$CAND_RUNTIME" ] || [ "$CAND_RUNTIME" = "$before" ]; then
    fail "the machine runs Runtime ${CAND_RUNTIME:-?} as before the candidates: the Runtime candidate did not go on"
  fi
  echo "the candidate Runtime: $CAND_RUNTIME (was ${before:-none})"
}

# Where this OS runs no models itself there is no machine to take candidates: a candidate cozy is this computer's, as
# `cozy update` puts one in place (an exact release version is that verb itself), a dev build's file swapped in as it
# swaps (renamed over; a running cozy.exe is set aside), and `cozy doctor` restarts Calcifer on it.
candidate_cozy() {
  local want dir
  [ -n "$CAND_COZY" ] || return 0
  if [ -f "$CAND_COZY" ]; then
    want=$("$CAND_COZY" --version) dir=$(dirname "$COZY")
    cp "$CAND_COZY" "$dir/.cozy-candidate" && chmod +x "$dir/.cozy-candidate" || fail "copying the candidate cozy failed"
    [ "$RUNNER_OS" != Windows ] || mv -f "$COZY" "$dir/.cozy-replaced-$RANDOM.exe"
    mv -f "$dir/.cozy-candidate" "$COZY" || fail "putting the candidate cozy in place failed"
  else
    want="cozy ${CAND_COZY#v}"
    cozy --json update "${CAND_COZY#v}" > "$OUT/update-candidate.json" || fail "cozy update ${CAND_COZY#v} failed: $(head -c 300 "$OUT/update-candidate.json")"
  fi
  [ "$(cozy --version)" = "$want" ] || fail "cozy is $(cozy --version), not the candidate $want"
  cozy --json doctor > "$OUT/doctor-candidate.json" || check "cozy doctor is not healthy after the candidate"
  grep -q "cozy ${want#cozy }," "$OUT/doctor-candidate.json" || check "Calcifer is not the candidate ${want#cozy }"
}

# Calcifer stuck: its process stopped (SIGSTOP), so it answers nothing and makes no progress. `cozy doctor`
# must say so, end it and start another, and the next run must go. Windows cannot stop a process this way.
calcifer_stuck() {
  [ "$RUNNER_OS" != Windows ] || return 0
  local pid
  pid=$(grep -o '"pid": *[0-9]*' "$HOME/.cozy/calcifer/calcifer.lock.holder" 2> /dev/null | grep -o '[0-9]*$')
  [ -n "$pid" ] || pid=$(cozy --json status | grep -o '"pid": *[0-9]*' | head -1 | grep -o '[0-9]*$')
  [ -n "$pid" ] || { check "no Calcifer pid to stop"; return 1; }
  kill -STOP "$pid" || { check "SIGSTOP $pid failed"; return 1; }
  cozy --json doctor > "$OUT/doctor-stuck.json" || check "cozy doctor did not fix a stuck Calcifer"
  [ "$RUNNER_OS" != Linux ] || journalctl --user -u cozy-calcifer.service -n 120 --no-pager > "$OUT/calcifer-journal.txt" 2>&1 || true
  grep -q '"fixed": *"restarted' "$OUT/doctor-stuck.json" || check "cozy doctor did not restart the stuck Calcifer"
  local stat; stat=$(ps -o stat= -p "$pid" 2> /dev/null || true)
  # Gone, or a zombie its service manager has yet to reap.
  [ -z "$stat" ] || [ "${stat#Z}" != "$stat" ] || check "the stuck Calcifer $pid still runs ($stat)"
}

# Calcifer stopped while the machine it started runs on (run 40071937261: the next Calcifer could not take
# the home until that machine idled out, 15 min). `cozy doctor` must recover at once: name what holds
# Calcifer's lock, end it, start Calcifer, and the next run must go.
calcifer_orphaned() {
  [ "$RUNNER_OS" != Windows ] || return 0
  local pid began
  machine_ready
  pid=$(grep -o '"pid": *[0-9]*' "$HOME/.cozy/calcifer/calcifer.lock.holder" 2> /dev/null | grep -o '[0-9]*$')
  [ -n "$pid" ] || { check "no Calcifer pid to stop"; return 1; }
  kill -TERM "$pid"
  for _ in $(seq 60); do kill -0 "$pid" 2> /dev/null || break; sleep 1; done
  ls -l /proc/*/fd 2> /dev/null | grep -c calcifer.lock > "$OUT/orphan-fds.txt" || true
  began=$(date +%s)
  "$COZY" --json doctor > "$OUT/doctor-orphaned.json" || check "cozy doctor did not recover Calcifer after an orphaned machine ($(( $(date +%s) - began )) s)"
  [ $(( $(date +%s) - began )) -lt 120 ] || check "cozy doctor took $(( $(date +%s) - began )) s to recover Calcifer"
  grep -o '"fixed": *"[^"]*"' "$OUT/doctor-orphaned.json" || true
}

# Removing cozy as the README's "Remove cozy" says, for this OS: nothing of cozy is left.
remove_cozy() {
  case $RUNNER_OS in
    Linux)
      systemctl --user disable --now cozy-calcifer cozy-machine > "$OUT/remove.log" 2>&1 || true
      pkill -f "$HOME/.cozy/bin/cozy" || true; sleep 2
      rm -rf ~/.cozy ~/.config/cozy ~/.local/share/bash-completion/completions/cozy ~/.config/fish/completions/cozy.fish ;;
    macOS)
      launchctl bootout "gui/$(id -u)/com.cozy-creator.calcifer" > "$OUT/remove.log" 2>&1 || true
      launchctl bootout "gui/$(id -u)/com.cozy-creator.machine" >> "$OUT/remove.log" 2>&1 || true
      pkill -f "$HOME/.cozy/bin/cozy" || true; sleep 2
      rm -rf ~/.cozy ~/.config/cozy ~/Library/LaunchAgents/com.cozy-creator.*.plist ;;
    Windows)
      powershell -NoProfile -c 'Remove-ItemProperty HKCU:\Software\Microsoft\Windows\CurrentVersion\Run cozy-calcifer, cozy-machine -ErrorAction SilentlyContinue; Get-Process cozy -ErrorAction SilentlyContinue | Stop-Process -Force; Start-Sleep 2; Remove-Item -Recurse -Force ~\.cozy, $env:LOCALAPPDATA\cozy -ErrorAction SilentlyContinue; $path = ([Environment]::GetEnvironmentVariable("Path", "User") -split ";") -ne "$HOME\.cozy\bin"; [Environment]::SetEnvironmentVariable("Path", $path -join ";", "User")' > "$OUT/remove.log" 2>&1 || true ;;
  esac
  [ ! -e "$COZY" ] || check "removing cozy left $COZY"
  [ ! -e "$HOME/.cozy" ] || check "removing cozy left ~/.cozy"
  ! pgrep -f "$HOME/.cozy/bin/cozy" > /dev/null 2>&1 || check "a cozy process still runs after removal"
}

# Whether this computer's cozy has `cozy machine <verb>`.
has_verb() { cozy machine --help | grep -E "^ +$1 " > /dev/null; }

# The local machine ends, as its idle exit ends it: the holder of its root's lock. The next use starts it.
# A cozy before cl-646 disables it instead (machine_ready enables it again).
machine_stop() {
  if has_verb disable; then cozy --json machine disable > "$OUT/disable.json"; return; fi
  local holder pid
  holder=$(find "$HOME/.cozy/machine" -name agent.lock.holder 2> /dev/null | head -1)
  find "$HOME/.cozy/machine" -name 'agent.lock*' > "$OUT/locks.txt" 2>&1 || true
  [ -n "$holder" ] || return 1
  cp "$holder" "$OUT/agent.lock.holder"
  pid=$(grep -o '"pid": *[0-9]*' "$holder" | grep -o '[0-9]*$') || return 1
  if [ "$RUNNER_OS" = Windows ]; then taskkill //F //T //PID "$pid"; else kill "$pid"; fi
  # Gone, or a zombie its parent has not reaped yet (kill -0 still finds one).
  for _ in $(seq 60); do
    if [ "$RUNNER_OS" = Windows ]; then kill -0 "$pid" 2> /dev/null || return 0
    else case $(ps -o stat= -p "$pid" 2> /dev/null || true) in '' | Z*) return 0 ;; esac; fi
    sleep 1
  done
  ps -o pid,stat,args -p "$pid" > "$OUT/stop.txt" 2>&1 || true
  return 1
}

# The process holding the local machine's root (on Windows its service, whose parent is TensorD's own).
machine_pid() { grep -o '"pid": *[0-9]*' "$HOME/.cozy/machine/var/lib/cozy/machine/agent.lock.holder" | grep -o '[0-9]*$'; }
parent_of() {
  if [ "$RUNNER_OS" = Windows ]; then
    powershell -NoProfile -c "(Get-CimInstance Win32_Process -Filter 'ProcessId=$1').ParentProcessId" | tr -d '\r '
  else
    ps -o ppid= -p "$1" | tr -d ' '
  fi
}
# Whether process $1 is gone, a zombie not: one answers kill -0 until its parent reaps it.
gone() {
  if [ "$RUNNER_OS" = Windows ]; then ! tasklist //FI "PID eq $1" | grep -q " $1 "; else ! kill -0 "$1" 2>/dev/null; fi
}

# cl-646: the local machine Calcifer started, ended, is reaped by Calcifer (gone, no zombie), and the next
# `machine show local` starts another. One the installer's `cozy-machine` service still runs is ended
# first, until the machine is the one Calcifer started.
machine_reaped() {
  local calcifer pid top='' i
  calcifer=$(cozy --json status | grep -o '"pid": *[0-9]*' | head -1 | grep -o '[0-9]*$')
  for i in 1 2 3; do
    machine_ready
    pid=$(machine_pid) || { check "no process holds the local machine's root"; return; }
    top=$pid
    [ "$RUNNER_OS" != Windows ] || top=$(parent_of "$pid")
    [ "$(parent_of "$top")" = "$calcifer" ] && break
    echo "the local machine (pid $top) is $(parent_of "$top")'s, not Calcifer's ($calcifer): ending it"
    machine_stop || true
  done
  [ "$(parent_of "$top")" = "$calcifer" ] || { check "Calcifer (pid $calcifer) never started the local machine"; return; }
  if [ "$RUNNER_OS" = Windows ]; then taskkill //F //T //PID "$top"; else kill "$top"; fi
  for i in $(seq 30); do gone "$top" && break; sleep 1; done
  gone "$top" || check "Calcifer left the local machine it started (pid $top) behind: state $(ps -o stat= -p "$top" 2>/dev/null)"
  machine_ready
  [ "$(machine_pid)" != "$pid" ] || check "the next machine show local did not start another machine"
  echo "the local machine Calcifer started (pid $top) was reaped; the next one is pid $(machine_pid)"
}

# cl-646: a Calcifer that stops (a service manager's SIGTERM, or killed) leaves the local machine it started
# running, and the next one adopts it at once: the next `cozy` command answers within seconds with the same
# machine.
calcifer_restart_adopts() {
  local pid how calcifer began took
  machine_ready
  pid=$(machine_pid) || { check "no process holds the local machine's root"; return; }
  for how in term kill; do
    [ $how = kill ] || [ "$RUNNER_OS" != Windows ] || continue
    calcifer=$(cozy --json status | grep -o '"pid": *[0-9]*' | head -1 | grep -o '[0-9]*$')
    case $how in
      term) kill -TERM "$calcifer" ;;
      kill) if [ "$RUNNER_OS" = Windows ]; then taskkill //F //PID "$calcifer"; else kill -9 "$calcifer"; fi ;;
    esac
    for _ in $(seq 30); do gone "$calcifer" && break; sleep 1; done
    began=$(date +%s)
    cozy --json machine show local > "$OUT/adopted-$how.json" 2>&1 || check "after $how: cozy machine show local failed"
    took=$(($(date +%s) - began))
    [ "$took" -le 30 ] || check "after $how: the next cozy command took ${took}s"
    [ "$(machine_pid)" = "$pid" ] || check "after $how: Calcifer started another machine, not adopting pid $pid"
    echo "after $how: the next Calcifer answered in ${took}s with the same machine (pid $pid)"
  done
}

# cl-646: an idle executor that is killed leaves TensorD's pool and is reaped at once: gone from ps within a
# second, never left a zombie until the next call. Linux and macOS (Windows has no zombies).
executor_reaped() {
  [ "$RUNNER_OS" != Windows ] || return 0
  local executor
  executor=$(pgrep -n -f "cozy_runtime.internal.executor")
  [ -n "$executor" ] || { check "no idle executor to end"; return 1; }
  kill -9 "$executor"
  sleep 1
  if ps -o stat= -p "$executor" > /dev/null 2>&1; then
    check "the killed idle executor (pid $executor) is still in ps a second later: $(ps -o stat= -p "$executor")"
    return 1
  fi
  echo "the killed idle executor (pid $executor) was gone from ps within a second"
}

# The local machine is up and reports itself (`machine show local` starts it); its record lands in
# $OUT/machine.json. A cozy before cl-646 (#1883; the installer's release until one carries it) makes its
# machine with `cozy machine enable`; a later one starts it on first use.
machine_ready() {
  if has_verb enable; then cozy --json machine enable > "$OUT/enable.json" || check "cozy machine enable failed"; fi
  # It starts on `machine show local`, which says its boot; a start that fails is the machine's typed error.
  while cozy --json machine show local > "$OUT/machine.json" 2> "$OUT/machine.err"; do
    grep -q '"state": *"ready"' "$OUT/machine.json" && return 0
    grep -qE '"state": *"(ending|ended)"' "$OUT/machine.json" && break
    sleep 5
  done
  cat "$OUT/machine.json" "$OUT/machine.err" || true
  fail "the local machine did not start: $(grep -o '"message": *"[^"]*"' "$OUT/machine.json" "$OUT/machine.err" | head -1)"
}

# cell <name> <target> [inputs...]: one `cozy run` on local (e2e/run.sh's run_cell), followed to its end. Its
# events go to $OUT/<name>.ndjson, its record (`cozy run show`) to $OUT/<name>.json, its number to
# $OUT/<name>.number, its outputs under $OUT/out/<name>/. A run that does not complete is a recorded failure
# (`check`), unless ALLOW names a code its refusal carries: then $OUT/<name>.refused holds that code. Returns 1
# when the run neither completed nor was refused as allowed.
RUN_CLI=cozy; . "$E2E/run.sh"
cell() {
  local name=$1 target=$2 code=0 n
  shift 2
  run_cell "$OUT" "$name" "$target" "$@" --on local || code=$?
  n=$(cat "$OUT/$name.number")
  if [ "$code" = 0 ]; then
    echo "$name: run $n completed"
    return 0
  fi
  tail -c 3000 "$OUT/$name.ndjson" || true
  tail -c 3000 "$OUT/$name.err" || true
  if [ -n "${ALLOW:-}" ] && grep -q "$ALLOW" "$OUT/$name.ndjson" "$OUT/$name.err"; then
    echo "$ALLOW" > "$OUT/$name.refused"
    echo "$name: run ${n:-none} refused with $ALLOW"
    return 0
  fi
  check "$name: cozy run $target did not complete (run $n $(run_get "$OUT" "$name" .status)): $(run_get "$OUT" "$name" '.reason.message' | head -c 300)$(grep -o '"message": *"[^"]*"' "$OUT/$name.ndjson" | head -1 || true)"
  return 1
}

# The first output file of a cell with this extension (its record's output[].path).
output() { run_output "$OUT" "$1" "$2"; }

# A background removal: the input's geometry, its background white, and its subject kept (a small subject is
# mostly white; an all-white image lost it).
white_background() {
  uv run -q --no-project --with pillow python - "$1" "$2" <<'PY'
import sys
from PIL import Image
image, source = Image.open(sys.argv[1]).convert("RGB"), Image.open(sys.argv[2])
assert image.size == source.size, (image.size, source.size)
white = sum(1 for p in image.getdata() if min(p) > 245) / (image.width * image.height)
print(f"white background {white:.2f}")
assert 0.1 < white < 0.98, white
PY
}

# Every input image the suites send: real photos, each run its own (BiRefNet memoizes a repeated input).
picture() {
  local file=$OUT/inputs/$1
  mkdir -p "$OUT/inputs"
  [ -f "$file" ] || curl -fsSL -o "$file" "https://raw.githubusercontent.com/pytorch/vision/v0.24.0/gallery/assets/$1"
  echo "$file"
}

# The BiRefNet target: a pinned release, the packages checkout's directory, or the published release.
birefnet() {
  local pin
  pin=$(tr ',' '\n' <<< "$PINS" | sed -n 's/^birefnet@//p' | head -1)
  if [ -n "$pin" ]; then echo "birefnet/remove_background@$pin"
  elif [ -n "$PACKAGES" ]; then echo "./$PACKAGES/birefnet/remove_background"
  else echo birefnet/remove_background; fi
}

echo "::group::start"
