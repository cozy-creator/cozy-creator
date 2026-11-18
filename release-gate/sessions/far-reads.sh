# far-reads (cl-1216): how a result and a machine's log reach this computer from a rental of $SKU, this runner's Calcifer its
# client. Read off the runner's own sockets (`ss`, every connection to the pod and the bytes it received) and the landed file's
# own times (its birth is when its read began, its last write when its last byte came). Paul's 26 MiB video came at 1.2 MB/s
# while a 16 MB log was read beside it.
#   1. lanes:  a 2 MiB and a 26 MiB result (fixtures/far-reads/made.py), each read cold (its lanes idle 95 s) and then twice
#              warm: the seconds its read took, the connections that carried it and how many were new, the round trip, and
#              when its first 64 KiB, its first MiB, a quarter and half of it had come. Where the first second goes.
#   2. logs:   the pod's TensorD log, grown to `log_mb` MB with its own lines, read whole three times: the seconds, and the
#              bytes the pod's connections brought for it.
#   3. beside: the 26 MiB result read three times alone, then three times with whole-log reads going beside it.
#   4. window: the log's last 30 s asked for (`--lines 0 --since 30s`): its text's bytes and the bytes brought for it.
# 2 to 4 run in each arm: `after`, this session's cozy here and on the pod, and with `base`, `before`, that build on both.
# Between the arms the builds are mixed both ways (the older Calcifer on the newer TensorD, then the reverse): each reads the log whole.
# With `police_mbit`, 2 and 3 run again with the pod's bytes into this runner policed to that rate (tc ingress): a slow link's
# stand-in, said as one. Nothing fails on a speed: it reports, and fails only when a read or an arm doesn't happen.
# params:
#   base         a cozy-creator dev-build run id of the build before the change (default: none, no `before` arm)
#   sizes        the results of 1, MiB (default [2, 26])
#   log_mb       the log's size, MB (default 16)
#   police_mbit  the stand-in link, Mbit/s (default 0: none)
#   seconds      what the cap must buy (default 1800)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/far-reads/made.py READ=$HERE/fixtures/far-reads/read.py
check() { say "FAIL $*"; RC=1; }
for tool in jq python3 ss ssh-keygen; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -f "$FIX" ] && [ -f "$READ" ] || fail "no fixtures/far-reads"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no dev build: dispatch with cozy=<a dev-build run of the change>"
BASE=$(param base '') LOG_MB=$(param log_mb 16) POLICE=$(param police_mbit 0)
[[ $LOG_MB =~ ^[0-9]+$ && $POLICE =~ ^[0-9]+$ ]] || fail "log_mb $LOG_MB, police_mbit $POLICE"
mapfile -t SIZES < <(param sizes '[2, 26]' | jq -r '.[]')
COZY_BIN=$(command -v cozy)
# The two builds, kept out of the session's published files.
BIN=${RUNNER_TEMP:-/tmp}/far-reads; mkdir -p "$BIN" || fail "no $BIN"
cp "$COZY_BIN" "$BIN/after-cozy" || fail "no copy of this session's cozy"
if [ -n "$BASE" ]; then
  [[ $BASE =~ ^[0-9]+$ ]] || fail "base: a cozy-creator dev-build run id"
  gh run download "$BASE" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$BIN/base-dl" > /dev/null 2>&1 || fail "no cozy in dev-build run $BASE"
  install -m 0755 "$(find "$BIN/base-dl" -name cozy-linux-x86_64 | head -1)" "$BIN/before-cozy" || fail "no cozy-linux-x86_64 in run $BASE"
  say "before: $("$BIN/before-cozy" --version), after: $(cozy --version)"
fi
# A dev home: its rental follows the open RC's builds (the Runtime an RC TensorD needs), as skew-upload's does.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
trap 'code=$?; claim; (exit "$code"); finish' EXIT
# ssh grows the pod's log with its own lines.
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param seconds 1800)"
GUARD_ALSO=own guard
rent
# Its boot update settled first.
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
IP=$(jq -r '.address // empty' "$OUT/show.json"); IP=${IP%:*}
[ -n "$IP" ] || fail "$NAME has no address: $(head -c 300 "$OUT/show.json")"
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@" < /dev/null; }

# sockets <file>: every connection from here to the pod now. sampling <file>: that every 50 ms, each under `@ <unix seconds>`.
sockets() { ss -tinH dst "$IP" > "$1" 2> /dev/null; }
SAMPLER=''
LOOP=''
sampling() { ( while :; do echo "@ $(date +%s.%N)"; ss -tinH dst "$IP" 2> /dev/null; sleep 0.05; done ) > "$1" 9>&- & SAMPLER=$!; BG="$SAMPLER $LOOP"; }
sampled() { [ -z "$SAMPLER" ] || kill "$SAMPLER" 2> /dev/null; SAMPLER=''; BG=$LOOP; }

# made <label> <MiB>: one result of that size made on the pod and landed here: its read's seconds in TOOK, and its line said.
SEED=$(date +%s) TOOK=''
made() {
  local label=$1 mib=$2 birth last bytes path
  SEED=$((SEED + 1)); TOOK=''
  sampling "$OUT/$label.ss"
  step "$label" run "$FIX/made" "mib:=$mib" "seed:=$SEED" --on "$NAME" || { sampled; check "$label: the run: $(tail -n 3 "$OUT/$label.txt" | paste -sd' ' | cut -c1-300)"; return 1; }
  for _ in $(seq 600); do
    read -r birth last bytes path < <(python3 -I "$READ" landed "$RUN_PROJECT" 2> "$OUT/$label.landed.err") && [ -n "${path:-}" ] && break
    sleep 0.2
  done
  sleep 0.3; sampled
  [ -n "${path:-}" ] || { check "$label: its video never landed: $(tail -c 200 "$OUT/$label.landed.err")"; return 1; }
  [ "${birth%%.*}" != 0 ] || { check "$label: this filesystem keeps no birth time"; return 1; }
  TOOK=$(awk -v a="$birth" -v b="$last" 'BEGIN { printf "%.2f", b - a }')
  [ "$TOOK" != 0.00 ] || { check "$label: its file's times don't span its read (born $birth, last written $last)"; return 1; }
  say "$label: $mib MiB ($bytes B) read in $TOOK s, $(awk -v n="$bytes" -v s="$TOOK" 'BEGIN { printf "%.2f", n / 1e6 / (s > 0 ? s : 0.001) }') MB/s; $(python3 -I "$READ" ramp "$OUT/$label.ss" "$birth" "$last")"
  # Kept: the lanes reads' samples, cut to their read.
  if [[ $label == lanes-* ]]; then python3 -I "$READ" trim "$OUT/$label.ss" "$birth" "$last"; else rm -f "$OUT/$label.ss"; fi
}

# grown: the pod's TensorD log at LOG_MB MB, its own lines again and again (gzip looks back 32 KiB: a copy further off packs no
# better than the lines themselves).
grown() {
  local script said
  script=$(cat <<EOS
d=/var/lib/cozy/machine/logs
f=\$(ls -t "\$d"/tensord.*.log 2> /dev/null | head -1)
[ -n "\$f" ] || { echo "no TensorD log in \$d"; exit 1; }
cp "\$f" /tmp/far-reads.lines || exit 1
[ -z "\$(tail -c 1 /tmp/far-reads.lines)" ] || echo >> /tmp/far-reads.lines
n=0
while [ "\$(stat -c %s "\$f")" -lt $((LOG_MB * 1000000)) ] && [ \$n -lt 4000 ]; do cat /tmp/far-reads.lines >> "\$f"; n=\$((n + 1)); done
echo "grown \$(stat -c %s "\$f") from \$(stat -c %s /tmp/far-reads.lines)"
EOS
)
  said=$(pod "echo $(base64 -w0 <<< "$script") | base64 -d | sh" 2>&1)
  grep -q '^grown ' <<< "$said" || fail "the pod's log wasn't grown: $(tail -c 300 <<< "$said")"
  say "the pod's TensorD log: $(grep '^grown ' <<< "$said") B of its own lines$([ "$(awk '/^grown /{print $4}' <<< "$said")" -ge 32768 ] || echo ' (under gzip'"'"'s 32 KiB look-back: its ratio flatters)')"
}

# logs <label> [times]: the whole log read three times: each one's seconds, its text's bytes and the bytes the pod's connections brought.
logs() {
  local i t0 t1 wire text try
  for i in $(seq "${2:-3}"); do
    # Read again when less than the grown log came: Calcifer answers its own few kept lines for a machine it hasn't reached yet.
    for try in 1 2 3 4 5 6; do
      sockets "$OUT/$1-log$i.before"; t0=$(date +%s.%N)
      cozy machine logs "$NAME" --lines 0 > "$OUT/$1-log.txt" 2> "$OUT/$1-log.err" || { check "$1: the log read: $(tail -c 300 "$OUT/$1-log.err")"; return 1; }
      t1=$(date +%s.%N); sockets "$OUT/$1-log$i.after"
      wire=$(python3 -I "$READ" wire "$OUT/$1-log$i.before" "$OUT/$1-log$i.after"); text=$(stat -c %s "$OUT/$1-log.txt"); rm -f "$OUT/$1-log.txt"
      [ "$text" -lt $((LOG_MB * 500000)) ] || break
      sleep 5
    done
    [ "$text" -ge $((LOG_MB * 500000)) ] || check "$1: the log came as $text B, not the $LOG_MB MB it holds"
    say "$1: the log whole, read $i: $(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.2f", b - a }') s, $text B of text, $wire B on the wire ($(awk -v t="$text" -v w="$wire" 'BEGIN { printf "%.1f", t / (w > 0 ? w : 1) }')x)"
  done
}

# window <label>: the log's last 30 s asked for: its text's bytes and the bytes the pod's connections brought.
window() {
  local wire text
  sockets "$OUT/$1-window.before"
  cozy machine logs "$NAME" --lines 0 --since 30s > "$OUT/$1-window.txt" 2> "$OUT/$1-window.err" || { check "$1: the window read: $(tail -c 300 "$OUT/$1-window.err")"; return 1; }
  sockets "$OUT/$1-window.after"
  wire=$(python3 -I "$READ" wire "$OUT/$1-window.before" "$OUT/$1-window.after"); text=$(stat -c %s "$OUT/$1-window.txt")
  say "$1: the log's last 30 s: $text B of text, $wire B on the wire"
}

# runner <cozy>: that build as this runner's cozy and Calcifer, the pod's log read through it again.
runner() {
  [ "$(cozy --version)" != "$("$1" --version)" ] || return 0
  install -m 0755 "$1" "$COZY_BIN" && cozy restart > "$OUT/restart-runner.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart-runner.txt")"
  for _ in $(seq 60); do
    [ "$(cozy machine logs "$NAME" --lines 400 2> /dev/null | wc -l)" -ge 300 ] && return 0
    sleep 2
  done
  fail "$NAME's log isn't read through $(cozy --version)"
}

# beside <label>: the 26 MiB result three times alone, then three times with whole-log reads going beside it.
beside() {
  local i alone=() with=()
  for i in 1 2 3; do made "$1-alone$i" 26 && alone+=("$TOOK"); done
  : > "$OUT/$1-beside.reads"
  ( while :; do cozy machine logs "$NAME" --lines 0 > /dev/null 2>&1 && echo "$(date +%s.%N)" >> "$OUT/$1-beside.reads" || sleep 1; done ) 9>&- & LOOP=$!
  BG=$LOOP; sleep 1
  for i in 1 2 3; do made "$1-beside$i" 26 && with+=("$TOOK"); done
  pkill -P "$LOOP" 2> /dev/null; kill "$LOOP" 2> /dev/null; wait "$LOOP" 2> /dev/null; LOOP='' BG=''
  say "$1: 26 MiB alone ${alone[*]:-none} s; beside log reads ${with[*]:-none} s ($(wc -l < "$OUT/$1-beside.reads") whole-log reads meanwhile)"
  [ "${#alone[@]}" = 3 ] && [ "${#with[@]}" = 3 ] || check "$1: not every read happened"
}

# police <Mbit/s|off>: the pod's bytes into this runner held to that rate, dropped past it (tc ingress), or let go.
DEV=$(ip -o route get "$IP" | sed -n 's/.* dev \([^ ]*\).*/\1/p')
police() {
  sudo tc qdisc del dev "$DEV" ingress 2> /dev/null
  [ "$1" != off ] || return 0
  sudo tc qdisc add dev "$DEV" handle ffff: ingress 2> "$OUT/tc.err" &&
    sudo tc filter add dev "$DEV" parent ffff: protocol ip u32 match ip src "$IP"/32 police rate "${1}mbit" burst 512k drop flowid :1 2>> "$OUT/tc.err"
}

# arm <label> <cozy>: that build as this runner's Calcifer and the pod's TensorD, then 2 and 3, and again policed.
arm() {
  local label=$1 bin=$2
  if [ "$(cozy --version)" != "$("$bin" --version)" ]; then
    install -m 0755 "$bin" "$COZY_BIN" && cozy restart > "$OUT/restart-$label.txt" 2>&1 || fail "$label: cozy restart: $(tail -c 300 "$OUT/restart-$label.txt")"
  fi
  step "update-$label" machine update "$NAME" --tensord "$bin" -y || fail "$label: the pod refused its TensorD"
  POD=$("$bin" --version | awk '{print $2}')
  for _ in $(seq 180); do
    cozy machine show "$NAME" --json > "$OUT/show-$label.json" 2> /dev/null
    [ "$(jq -r '.state // empty' "$OUT/show-$label.json")" = ready ] && break
    sleep 5
  done
  [ "$(jq -r '.status.version // empty' "$OUT/show-$label.json")" = "$("$bin" --version | awk '{print $2}')" ] ||
    fail "$label: the pod runs $(jq -r '.status.version' "$OUT/show-$label.json"), not $("$bin" --version)"
  say "$label: Calcifer $(cozy --version | awk '{print $2}'), TensorD $(jq -r '.status.version' "$OUT/show-$label.json")"
  made "$label-first" 0.25 || fail "$label: the fixture didn't run"
  if [ "$label" = after ]; then
    # 1. lanes: cold, then warm.
    local mib i
    for mib in "${SIZES[@]}"; do
      say "lanes: $mib MiB, its lanes left idle 95 s"; sleep 95
      made "lanes-$mib-cold" "$mib"
      for i in 1 2; do made "lanes-$mib-warm$i" "$mib"; done
    done
    grown
  fi
  logs "$label"
  beside "$label"
  window "$label"
  [ "$POLICE" != 0 ] || return 0
  if police "$POLICE"; then
    say "$label: the pod's bytes into this runner policed to $POLICE Mbit/s"
    logs "$label-policed"
    beside "$label-policed"
  else
    say "$label: no ingress policing on this runner ($(tail -c 200 "$OUT/tc.err" | paste -sd' ')): the stand-in link is left out"
  fi
  police off
}

POD=''
arm after "$BIN/after-cozy"
if [ -n "$BASE" ]; then
  runner "$BIN/before-cozy"; say "mixed: Calcifer $(cozy --version | awk '{print $2}'), TensorD $POD"; logs mixed-older-calcifer 1
  arm before "$BIN/before-cozy"
  runner "$BIN/after-cozy"; say "mixed: Calcifer $(cozy --version | awk '{print $2}'), TensorD $POD"; logs mixed-newer-calcifer 1; window mixed-newer-calcifer
fi
cozy machine logs "$NAME" tensord --since 30m -n 2000 > "$OUT/tensord.log" 2>&1
{ echo "## far-reads on $NAME ($SKU), $IP"; grep -E ' (lanes|after|before|mixed)[a-z0-9-]*: |the pod.s TensorD log|FAIL' "$R" | sed 's/^/- /'; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
