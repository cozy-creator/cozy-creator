# probes (cl-774 Part E, cl-775, cl-633): two measurements on one cpu pod with SSH, on the session's cozy sent with `--tensord`. It builds nothing.
#   memory   from a real executor (a run's own process) and from the machine's shell: can it read TensorD's memory? fixtures/probes/fixture.py says
#            exactly what it touches: at most four bytes of a mapping's start, and whether they're an ELF header.
#   webrtc   a playback API link on its published HTTPS origin, on a real output of a real run, in Chromium, Firefox and WebKit on this runner
#            (fixtures/probes/webrtc.js). Playwright's WebKit on Linux isn't Safari: nothing here speaks for Safari or iOS.
# Before any rental, at $0, whichever `rent` says, all of it on this runner's own machine, so a pod's first answer isn't when it's learned that it doesn't work.
# params:
#   megabytes  the output's size (default 100)
#   rehearse   false skips the $0 rehearsal, once a pre-flight has passed it (default true)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures/probes" && pwd) || fail "no fixtures"
MB=$(param megabytes 100) REHEARSE=$(param rehearse true)
J() { jq -e "$@" > /dev/null; }
check() { say "FAIL: $*"; echo "::error::$*"; RC=1; }
mark() { say "$*"; echo "::notice::$*"; }
for tool in jq node npm curl python3 sudo ss iptables tc; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -f ~/.ssh/id_ed25519.pub ] || { mkdir -p ~/.ssh && ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519; } || fail "no SSH key"
RENT_ARGS=--ssh

# What this runner's Calcifer ordered is this session's (another client's rentals read `ordered: false`), so a lost answer's order is
# ended too.
trap 'code=$?; claim; (exit "$code"); finish' EXIT

PW=$(mktemp -d)
npm install -s --prefix "$PW" playwright-core@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install playwright-core: $(tail -c 300 "$OUT/npm.log")"
node "$PW/node_modules/playwright-core/cli.js" install --with-deps chromium firefox webkit > "$OUT/browsers.log" 2>&1 || fail "the browsers: $(tail -c 300 "$OUT/browsers.log")"

# there <machine> <script>: a shell on it, the script's stdin its own.
there() {
  if [ "$1" = local ]; then timeout 300 bash -c "$2"; else timeout 300 cozy machine ssh "$1" -- "$2"; fi
}

# memory <machine> <label>: the probe from an executor, then from a shell.
memory() {
  local x=$1 l=$2 n path=''
  step "memread-$l" run "$FIX/fixture.py/memread" "note=$l $RANDOM" --on "$x" || { check "memread on $x: $(tail -3 "$OUT/memread-$l.txt")"; return 1; }
  n=$(runno "memread-$l")
  for _ in $(seq 60); do
    path=$(cozy --json --full run watch "$n" --once | jq -r '.output[] | select(.name == "report") | .path // empty')
    [ -n "$path" ] && break
    sleep 1
  done
  [ -n "$path" ] && jq . "$path" > "$OUT/memory-executor-$l.json" || { check "memread on $x left no report"; return 1; }
  sed '/^# end of the probe$/q' "$FIX/fixture.py" | there "$x" 'python3 -I -' > "$OUT/memory-shell-$l.json" 2>> "$OUT/shell.err" || check "the probe in a shell on $x: $(tail -c 300 "$OUT/shell.err")"
  mark "memory, $l, from an executor: $(jq -c '{uid: .self.uid, ptrace: .self.sys_ptrace, yama: .ptrace_scope, provider: .provider_names_present, control: .control_own_memory.proc_mem.is_elf_header, tensord: [.tensord[] | {uid: .Uid, ancestor: .is_ancestor, mem: .attempt.proc_mem, vm: .attempt.process_vm_readv, nobody: .as_nobody}]}' "$OUT/memory-executor-$l.json")"
}

# speed <machine> <label>: a real output, the same bytes over HTTPS for what the path itself can carry, then the browsers.
speed() {
  local x=$1 l=$2 n link api cap id holder=$HOME/.cozy/calcifer/calcifer.lock.holder
  step "blob-$l" run "$FIX/fixture.py/blob" "megabytes:=$MB" --on "$x" || { check "blob on $x: $(tail -3 "$OUT/blob-$l.txt")"; return 1; }
  n=$(runno "blob-$l")
  id=$(cozy --json run watch "$n" --once | jq -r .id)
  link=$(curl --fail-with-body -sS -G -H "Authorization: Bearer $(jq -r .token "$holder")" \
    --data-urlencode output=blob --data-urlencode expires=2h "http://$(jq -r .address "$holder")/v1/runs/$id/play" | jq -r '.url // empty')
  [ -n "$link" ] || { check "playback API for $n on $x named no link"; return 1; }
  [[ $link == https://* ]] || { check "the link's origin isn't HTTPS: ${link%%#*}"; return 1; }
  say "player origin ${link%%#*}; the machine says $(cozy machine show "$x" --json | jq -c '.status.webrtc | {addresses, port, unavailable_reason}')"
  api=$(cozy machine show "$x" --json | jq -r .address)
  cap=$(sed -n 's/.*[#&]c=\([^&]*\).*/\1/p' <<< "$link") id=$(sed -n 's/.*[#&]r=\([^&]*\).*/\1/p' <<< "$link")
  for _ in 1 2; do
    curl -k -sS -o /dev/null -w '%{http_code} connect %{time_connect}s first byte %{time_starttransfer}s total %{time_total}s %{speed_download} B/s\n' \
      -H "Authorization: Cozy-Cap $cap" "https://$api/v1/runs/$id/outputs/blob" 2>&1 | tee -a "$OUT/https-$l.txt" | sed "s/^/https, $l: /" | tee -a "$R"
  done
  MACHINE=$x NET=$FIX/net.sh PLAYWRIGHT=$PW/node_modules/playwright-core timeout 2400 node "$FIX/webrtc.js" "$link" "$OUT/webrtc-$l.json" 2> "$OUT/webrtc-$l.err" | tee -a "$R"
  [ "${PIPESTATUS[0]}" = 0 ] || check "the browsers on $x: $(tail -c 300 "$OUT/webrtc-$l.err")"
}

if [ "$REHEARSE" = true ]; then
  mark "rehearsal on this runner's own machine, at no cost"
  memory local rehearsal || fail "the memory probe's rehearsal"
  J '.control_own_memory.proc_mem.is_elf_header and .control_own_memory.process_vm_readv.is_elf_header' "$OUT/memory-executor-rehearsal.json" || fail "the probe can't read even its own memory, so a no would prove nothing"
  speed local rehearsal || fail "the browsers' rehearsal"
  J '.chromium | (.opens.n == 5) and (.bulk.sha256_matches == true) and (.smalls.n == 100) and (.under_load.small.n > 0) and (.drop_reset.sha256_matches == true) and (.drop_blip_3s.error == null) and (.drop_blackhole.error == null)' "$OUT/webrtc-rehearsal.json" || fail "the harness doesn't work in Chromium here: $(jq -c '.chromium' "$OUT/webrtc-rehearsal.json" | head -c 800)"
  [ "$RC" = 0 ] || fail "the rehearsal failed: no rental"
  mark "rehearsal passed"
fi

# A cpu pod leaves the catalog and returns by the minute: the wait is here, before anything is ordered and where it costs nothing.
for try in $(seq 120); do
  cozy machine rent "$SKU" --disk-gb "$DISK" $RENT_ARGS --json < /dev/null > "$OUT/offer.json" 2>&1
  grep -q quote_unagreed "$OUT/offer.json" && break
  [ "$try" != 1 ] || say "no $SKU on offer yet"
  sleep 30
done
# A boot, the update, a Runtime's first run, three browsers: under half an hour. Some 900 MB each way for a browser, and the runs' landing.
quote 1800

( until [ -s "$OUT/rental" ]; do sleep 1; done; echo "::notice::rental $(head -1 "$OUT/rental")" ) & BG="$BG $!"
rent
jq -e '.rental.ssh // empty' "$OUT/show.json" > /dev/null || fail "the rental has no ssh address"
mark "rental $RID is $NAME: $(jq -c '{price: .rental.usd_micros_per_hour, note: .rental.placement_note}' "$OUT/show.json")"
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update: $(tail -3 "$OUT/update.txt")"
want=$(cozy --version | awk '{print $2}')
[ "$(cozy machine show "$NAME" --json | jq -r .status.version)" = "$want" ] || fail "the pod's TensorD isn't $want"
memory "$NAME" pod
speed "$NAME" pod
cozy machine show "$NAME" --json | jq -c '.rental | {spend_usd_micros, spend_basis, usd_micros_per_hour, rented_at_ms}' > "$OUT/spend.json" 2>&1
say "spend so far: $(cat "$OUT/spend.json")"
exit "$RC"
