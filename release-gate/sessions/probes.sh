# probes (cl-774 Part E): the two probes that decide whether the WebRTC tunnel goes ahead. One CPU
# rental, SSH, the session's dev cozy sent to it with --tensord. It measures; it builds no tunnel.
#
#   Probe 1 (cl-775): with an executor's privileges on this pod, can code read another process's
#   memory? From root (an executor runs as TensorD's own UID, which on a rental is root) and from a
#   fresh non-root user (the proposed fix's effect), it opens /proc/<tensord>/mem and tries
#   process_vm_readv. The check is benign: it reads four bytes at the start of the target's own
#   executable mapping and compares them to that file's ELF magic on disk. It never scans memory,
#   never reads any other region, and prints no byte it did not already know. It also records the
#   provider (the key is RunPod's), both UIDs, ptrace_scope, the container's capabilities and the
#   /proc/<pid>/mem ownership as the outside proxy for the target's dumpable bit.
#
#   Probe 2 (cl-633): WebRTC-direct from a real HTTPS origin (the published player at github.io), in
#   Chromium, Firefox and WebKit on the runner, driving the real cozy-webrtc.js against the rental's
#   WebRTC listener over the public internet. Per engine: connects at all (and any mixed-content/CSP
#   refusal), time to open, a 100 MB read, a hundred small reads, small reads during a bulk read, a
#   reconnect. It runs only if the Hub granted and mapped the pod's WebRTC port; if not, it says so.
#   WebKit on Linux is not Safari; Safari and iOS are for the owner, not a runner.
#
# params: mib (the blob output's size, default 100). Without rent=true it stops at the $0 pre-flight.
. "$(dirname "$0")/lib.sh"
MIB=$(param mib 100)
FIX=$(cd "$(dirname "$0")/fixtures/probes" && pwd) || fail "no fixtures"
PAGE=https://cozy-creator.github.io/cozy-creator/play/
J() { jq -e "$@" > /dev/null; }
check() { say "FAIL: $*"; echo "::error::$*"; RC=1; }
mark() { say "$*"; echo "::notice::$*"; }
for tool in jq node npm curl python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -f ~/.ssh/id_ed25519.pub ] || { mkdir -p ~/.ssh && ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519; } || fail "no SSH key"
RENT_ARGS=--ssh

pod() { timeout "${T:-300}" cozy machine ssh "$NAME" -- "$1" < /dev/null 2>> "$OUT/pod.err"; }
put() { timeout 120 cozy machine ssh "$NAME" -- "cat > $2" < "$1" 2>> "$OUT/pod.err"; }

# The $0 pre-flight ends here unless rent=true: it checks the dev cozy, parses params and does the
# cap arithmetic. A CPU rental's traffic is small; the guard buys the boot, both probes and the end.
cozy --version > "$OUT/version.txt" 2>&1 || fail "the session cozy does not run: $(tail -c 300 "$OUT/version.txt")"
[ "$MIB" -ge 1 ] 2> /dev/null || fail "mib must be a positive integer, not $MIB"
quote 2 1500

( until [ -s "$OUT/rental" ]; do sleep 1; done; echo "::notice::rental $(head -1 "$OUT/rental")" ) & BG="$BG $!"
rent
jq -e '.rental.ssh // empty' "$OUT/show.json" > /dev/null || fail "the rental has no ssh address"
X=$NAME
mark "rental $RID is $NAME"

# The pod's own provider and placement, from the rental record the Hub gave.
PROVIDER=$(jq -r '.rental.provider // empty' "$OUT/show.json")
mark "placement: $(jq -rc '{sku: .rental.sku, note: .rental.placement_note, image: .rental.base_worker_image_digest}' "$OUT/show.json")"

step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update: $(tail -3 "$OUT/update.txt")"
want=$(cozy --version | awk '{print $2}')
[ "$(cozy machine show "$NAME" --json | jq -r .status.version)" = "$want" ] || fail "the pod's TensorD is not $want after the update"
TPID=$(cozy machine show "$NAME" --json | jq -r '.status.tensord_pid // empty')
[ -n "$TPID" ] || fail "status names no tensord_pid"
pod "mkdir -p /root/probes" || fail "no shell on the pod: $(tail -c 300 "$OUT/pod.err")"

# ---- Probe 1: cl-775 ----
mark "Probe 1 (cl-775): reading another process's memory from an executor's privileges, TensorD pid $TPID"
# The provider the pod is on, and the WebRTC mapping, from non-secret launch-environment names only.
# The provider KEY is never named here, read, printed or used.
pod "tr '\0' '\n' < /proc/$TPID/environ 2>/dev/null | grep -E '^(RUNPOD_POD_ID|CONTAINER_ID|RUNPOD_PUBLIC_IP|PUBLIC_IPADDR|COZY_WEBRTC_INTERNAL_PORT)=' ; tr '\0' '\n' < /proc/$TPID/environ 2>/dev/null | grep -E '^(RUNPOD_TCP_PORT_|VAST_TCP_PORT_)' " > "$OUT/pod-env.txt" 2>> "$OUT/pod.err"
grep -q '^RUNPOD_POD_ID=' "$OUT/pod-env.txt" && PODPROV=runpod || { grep -q '^CONTAINER_ID=' "$OUT/pod-env.txt" && PODPROV=vast || PODPROV=unknown; }
[ -n "$PROVIDER" ] || PROVIDER=$PODPROV
mark "the pod is on: $PROVIDER (from the launch environment's id name)"
cat "$OUT/pod-env.txt" | sed 's/^/    env /' >> "$R"

# A real executor's UID beside TensorD's: a quick run leaves a warm executor to scan for.
step blob-probe1 run "$FIX/blob.py/blob" "mib:=1" --on "$NAME" || say "note: the probe-1 warm-up run did not complete; the UID scan may be empty"
pod "echo tensord_uid=\$(awk '/^Uid:/{print \$2}' /proc/$TPID/status); for p in /proc/[0-9]*; do ppid=\$(awk '/^PPid:/{print \$2}' \$p/status 2>/dev/null); [ \"\$ppid\" = \"$TPID\" ] || continue; echo child \$(basename \$p) uid=\$(awk '/^Uid:/{print \$2}' \$p/status 2>/dev/null) cmd=\"\$(tr '\0' ' ' < \$p/cmdline 2>/dev/null | cut -c1-70)\"; done" > "$OUT/uids.txt" 2>> "$OUT/pod.err"
cat "$OUT/uids.txt" | sed 's/^/    /' >> "$R"

put "$FIX/mem.py" /root/probes/mem.py || check "copying mem.py to the pod"
# As root: the privilege a package's executor runs with on a rental today (same UID as TensorD).
pod "python3 /root/probes/mem.py $TPID" > "$OUT/mem-root.json" 2>> "$OUT/pod.err"
J . "$OUT/mem-root.json" || check "the root memory probe printed no JSON: $(tail -c 300 "$OUT/mem-root.json") $(tail -c 200 "$OUT/pod.err")"
# As a fresh non-root user: the effect the proposed fix (executors as another user) would have.
pod "id cozyexec >/dev/null 2>&1 || useradd -m cozyexec; install -m 0755 /root/probes/mem.py /home/cozyexec/mem.py; sudo -u cozyexec python3 /home/cozyexec/mem.py $TPID" > "$OUT/mem-otheruser.json" 2>> "$OUT/pod.err"
J . "$OUT/mem-otheruser.json" || say "note: the non-root memory probe printed no JSON (recorded as a run note): $(tail -c 200 "$OUT/mem-otheruser.json")"

ROOT_READ=$(jq -r '.memory_readable // "?"' "$OUT/mem-root.json" 2>/dev/null)
OTHER_READ=$(jq -r '.memory_readable // "?"' "$OUT/mem-otheruser.json" 2>/dev/null)
PTRACE=$(jq -r '.ptrace_scope // "?"' "$OUT/mem-root.json" 2>/dev/null)
mark "Probe 1 verdict: executor-privilege (root) read = $ROOT_READ; non-root user read = $OTHER_READ; ptrace_scope = $PTRACE; provider = $PROVIDER"
[ "$ROOT_READ" = true ] && mark "cl-775: YES on $PROVIDER — a package's executor (root, same UID as TensorD) can read TensorD's memory on this pod. Smallest fix: run rental executors as a dedicated non-root user (device_executor already pins them to TensorD's UID; make that UID not root), and/or set TensorD PR_SET_DUMPABLE(0). The non-root column above shows what that closes."

# ---- Probe 2: cl-633 WebRTC from a real HTTPS origin ----
WEBRTC_ADDR=$(cozy machine show "$NAME" --json | jq -r '.status.webrtc.addresses[0] // empty')
if [ -z "$WEBRTC_ADDR" ]; then
  mark "Probe 2 SKIPPED: the pod does not listen for players. status.webrtc=$(cozy machine show "$NAME" --json | jq -rc '.status.webrtc') — the Hub granted/mapped no WebRTC port on this $PROVIDER CPU rental (COZY_WEBRTC_INTERNAL_PORT / *_TCP_PORT_* above). The WebRTC-from-HTTPS numbers need a listening, mapped pod; recorded as unmeasured, not as a failure."
else
  mark "Probe 2: the pod listens for players at $WEBRTC_ADDR; measuring from $PAGE"
  step blob run "$FIX/blob.py/blob" "mib:=$MIB" --on "$NAME" || check "the blob run did not complete: $(tail -3 "$OUT/blob.txt")"
  N=$(cozy --json run ls --on "$NAME" --limit 1 2>/dev/null | jq -r '.runs[0].number // empty')
  [ -n "$N" ] || check "no run number for the blob run"
  LEN=$(cozy --json --full run show "$N" --once 2>/dev/null | jq -r '[.output[] | select(.name=="blob") | .length] | .[0] // empty')
  [ -n "$LEN" ] || check "the blob output has no length: $(cozy --json --full run show "$N" --once 2>/dev/null | jq -c '.output')"
  cozy --json run play "$N" --output blob --expires 1h > "$OUT/play.json" 2>&1 || check "run play: $(tail -c 300 "$OUT/play.json")"
  LINK=$(jq -r '.link // empty' "$OUT/play.json")
  [ -n "$LINK" ] && [[ $LINK == "$PAGE"* ]] || check "run play gave no github.io link: $(head -c 300 "$OUT/play.json")"
  # The three engines, on the runner, against the real published page.
  PW=$(mktemp -d)
  mark "installing playwright chromium, firefox, webkit on the runner"
  ( cd "$PW" && npm init -y > /dev/null 2>&1 && npm install -s playwright@1.55.0 > "$OUT/pw-npm.log" 2>&1 ) || check "npm install playwright: $(tail -c 300 "$OUT/pw-npm.log")"
  PLAYWRIGHT_BROWSERS_PATH="$PW/.browsers" npx --prefix "$PW" --yes playwright@1.55.0 install --with-deps chromium firefox webkit > "$OUT/pw-install.log" 2>&1 || check "playwright install: $(tail -c 400 "$OUT/pw-install.log")"
  if [ -n "$LINK" ]; then
    PLAYWRIGHT="$PW/node_modules/playwright" PLAYWRIGHT_BROWSERS_PATH="$PW/.browsers" PAGE="$PAGE" LINK="$LINK" BLOB_LEN="$LEN" OUT="$OUT" \
      timeout 900 node "$FIX/webrtc.js" > "$OUT/webrtc.json" 2> "$OUT/webrtc.err" || check "the webrtc harness: $(tail -c 400 "$OUT/webrtc.err")"
    for e in chromium firefox webkit; do
      f="$OUT/webrtc-$e.json"
      [ -f "$f" ] || { check "$e: no result file"; continue; }
      conn=$(jq -r '.connected // false' "$f"); open=$(jq -r '.open_ms // "?"' "$f"); mbps=$(jq -r '.steps.bulk.MBps // "?"' "$f")
      sm=$(jq -rc '.steps.small_reads // {}' "$f"); dur=$(jq -rc '.steps.small_during_bulk // {}' "$f"); rec=$(jq -r '.steps.reconnect_ms // (.steps.reconnect|tojson) // "?"' "$f")
      csp=$(jq -rc '.console // []' "$f")
      mark "$e: connected=$conn open=${open}ms bulk=${mbps}MB/s small=$sm during_bulk=$dur reconnect=${rec}ms console=$csp"
    done
    mark "100 MB UPLOAD is not measurable here: cozy/1 is download-only (follow/get); an upload needs the §8 tunnel's req/fin body path, which this probe does not build. Recorded as not-measured."
    mark "WebKit above is Playwright's Linux WebKit, NOT Safari. Safari and iOS are untested here by design."
  fi
fi

[ "$RC" = 0 ] && mark "probes done" || mark "probes done with recorded FAILs (see above)"
exit "$RC"
