# e2e/cli/rental.sh: rentals on the isolated Hub's docker provider (a container on this runner, no real pod), and sign-in
# by the code that development Hub logs instead of emailing (e2e/isolated.sh exports its log and email). Linux. A pod
# reaches the Hub, on this computer's loopback, only through forward.py (pod_reach); rental_end's never does, and stays
# booting.
#   rental_end   cl-875: `machine end` says a rental is shut down only once its Hub affirms it ended. Signed out, or
#                holding a key the Hub revoked (11-11: another computer's `logout` revoked a copied key), it exits non-zero
#                naming why, and the rental is still there; signed in again, it ends it. `rent --ssh` with no path takes
#                the default key, and `machine ssh` says what stands in the way: not ready yet, or signed out (not "not
#                a rental"). `logout` revokes a key this computer enrolled, never one installed with `login --key`,
#                which another computer may hold; a pod signs in with a key of its own (`login --export`).
#   pod_release  cl-875, cl-880: a pod that says it released itself is ended only on its Hub's word. Kept from its Hub
#                as its idle window ends (its release never lands, its Hub bills on) while this computer is signed out,
#                it shows not shut down and possibly billing, its container up; signed in, its end releases it at the
#                Hub and the container goes. The pod reaches the loopback Hub through forward.py.

# signin <cozy login args...>: `cozy login`, answered with the newest code the Hub logs after it starts. The Hub spaces
# enrollments to one email 30 s apart: refused that, it waits as told and asks again.
signin() {
  local out=$OUT/signin.json seen wait try
  for try in 1 2 3; do
    seen=$(grep -c 'development device enrollment code' "$E2E_HUB_LOG" || true)
    : > "$out"
    {
      for _ in $(seq 90); do
        code=$(grep 'development device enrollment code' "$E2E_HUB_LOG" | tail -n +$((seen + 1)) | grep -o '"code":"[^"]*"' | tail -1 | cut -d'"' -f4 || true)
        [ -z "$code" ] || { printf '%s\n' "$code"; break; }
        grep -q '"error"' "$out" 2> /dev/null && break
        sleep 1
      done
    } | cozy --json login "$@" > "$out" 2> "$out.err" && { cat "$out"; return 0; }
    wait=$(jq -r '.error.retry_after_s // empty' "$out" 2> /dev/null || true)
    [ -n "$wait" ] && [ "$try" -lt 3 ] || break
    echo "signin: the Hub asks for ${wait} s between codes" >&2
    sleep $((wait + 1))
  done
  cat "$out" "$out.err"
  return 1
}

rental_end() {
step "a rental's end, signed out and with a refused key (cl-875)"
[ -n "${E2E_HUB_LOG:-}" ] && [ -n "${E2E_HUB_EMAIL:-}" ] || { check "rental_end: e2e/isolated.sh named no Hub log or email to sign in with"; return; }
local keys=$HOME/.cozy/keys key rental pid code k1 k2 pod gone held
# Key files stay out of $OUT, which is published.
held=$(mktemp -d)
signin "$E2E_HUB_EMAIL" > "$OUT/rental-login.json" || { check "rental_end: cozy login failed: $(tail -c 300 "$OUT/rental-login.json")"; return; }
k1=$(jq -r '.device_key // empty' "$OUT/rental-login.json" || true)
key=$(find "$keys" -name 'device-*.json' ! -name '*.bearer.json' | head -1)
[ -n "$key" ] && [ -n "$k1" ] || { check "rental_end: signed in, but no device key: $(ls "$keys" 2>&1)"; return; }
# What the 11-11 agent copied to a pod: this computer's own key file.
cp "$key" "$held/laptop-key.json"

# `--ssh` with no path: the default key. The order stands once its Hub accepts it, which an end needs, not a ready
# machine. The follower is killed outright, as a closed terminal kills it: it ends nothing.
[ -f "$HOME/.ssh/id_ed25519.pub" ] || ssh-keygen -q -t ed25519 -N '' -f "$HOME/.ssh/id_ed25519"
cozy --json machine rent cpu --ssh -y > "$OUT/rental-rent.ndjson" 2> "$OUT/rental-rent.err" & pid=$!
for _ in $(seq 180); do
  rental=$(jq -r 'select(.type? == "rental.accepted") | .rental_id // empty' "$OUT/rental-rent.ndjson" 2> /dev/null | tail -1 || true)
  [ -n "$rental" ] || ! kill -0 "$pid" 2> /dev/null || { sleep 1; continue; }
  break
done
kill -KILL "$pid" 2> /dev/null || true
wait "$pid" 2> /dev/null || true
[ -n "$rental" ] || { check "rental_end: machine rent cpu: no rental accepted: $(tail -c 400 "$OUT/rental-rent.ndjson") $(tail -c 300 "$OUT/rental-rent.err")"; cozy --json logout > /dev/null 2>&1 || true; return; }
echo "rented $rental"

step "machine ssh: rented with the default key, not ready yet"
# Its Hub's word names the key at once; its SSH address comes once it is ready.
for _ in $(seq 60); do
  cozy --json machine show "$rental" > "$OUT/rental-show-ssh.json" 2> /dev/null && J '.rental != null' "$OUT/rental-show-ssh.json" && break
  sleep 1
done
J '.rental.ssh_key == true' "$OUT/rental-show-ssh.json" || check "rent --ssh with no path: the rental was not rented with a key: $(head -c 400 "$OUT/rental-show-ssh.json")"
if J '.rental.ssh == null' "$OUT/rental-show-ssh.json"; then
  code=0; cozy --json machine ssh "$rental" > "$OUT/rental-ssh-booting.json" 2> "$OUT/rental-ssh-booting.err" || code=$?
  [ "$code" != 0 ] && J '.error.code == "machine_ssh_booting"' "$OUT/rental-ssh-booting.json" \
    || check "machine ssh while it boots: exit $code, not 'not ready yet': $(head -c 300 "$OUT/rental-ssh-booting.json")"
else
  echo "it was ready before machine ssh was asked: $(jq -r .rental.ssh "$OUT/rental-show-ssh.json")"
fi

# end_refused <case> <words>: `machine end` exits non-zero, `end_failed` naming why (<words>), and the rental is still
# listed and not ended, its end failure shown.
end_refused() {
  code=0; cozy --json machine end "$rental" --force -y > "$OUT/rental-end-$1.json" 2> "$OUT/rental-end-$1.err" || code=$?
  [ "$code" != 0 ] && J --arg w "$2" '.error.code == "end_failed" and (.error.message | test("not shut down") and test($w))' "$OUT/rental-end-$1.json" \
    || check "machine end $1: exit $code, not a refusal naming '$2': $(tail -c 400 "$OUT/rental-end-$1.json")"
  ! grep -q 'shut down\.' "$OUT/rental-end-$1.json" "$OUT/rental-end-$1.err" || check "machine end $1 said the rental shut down: $(tail -c 300 "$OUT/rental-end-$1.json")"
  cozy --json machine show "$rental" > "$OUT/rental-show-$1.json" 2> "$OUT/rental-show-$1.err" \
    && J '.state != "ended" and (.end_failed | length > 0)' "$OUT/rental-show-$1.json" \
    || check "machine show after the refused end ($1): not live with its end failure: $(tail -c 400 "$OUT/rental-show-$1.json")"
}

step "machine end and machine ssh, signed out"
# This computer's key, enrolled here, is revoked at the Hub by its logout.
cozy --json logout > "$OUT/rental-logout.json" 2> "$OUT/rental-logout.err" || check "logout failed: $(tail -c 300 "$OUT/rental-logout.json")"
end_refused signed-out 'not signed in'
# Calcifer started again, signed out, holds no word from the Hub about it: `machine ssh` says why (cl-875: it said
# "not a rental").
systemctl --user restart cozy-calcifer || check "rental_end: systemctl --user restart cozy-calcifer failed"
code=0; cozy --json machine ssh "$rental" > "$OUT/rental-ssh-signed-out.json" 2> "$OUT/rental-ssh-signed-out.err" || code=$?
[ "$code" != 0 ] && J '.error.code == "not_logged_in" and (.error.message | test("not signed in"))' "$OUT/rental-ssh-signed-out.json" \
  || check "machine ssh signed out: exit $code, not 'not signed in': $(head -c 300 "$OUT/rental-ssh-signed-out.json")"

step "machine end, holding a key the Hub revoked"
# The laptop on 11-11: it still holds its key, which another computer's logout revoked.
install -m 600 "$held/laptop-key.json" "$key"
cozy --json whoami > "$OUT/rental-whoami-refused.json" 2> "$OUT/rental-whoami-refused.err" && J '.logged_in == false and .key_refused == true' "$OUT/rental-whoami-refused.json" \
  || check "whoami with a revoked key: not 'no longer accepted': $(tail -c 300 "$OUT/rental-whoami-refused.json")"
end_refused key-refused 'hub_credential'

step "machine end, signed in again"
cozy --json logout > /dev/null 2>&1 || true
signin "$E2E_HUB_EMAIL" > "$OUT/rental-login-2.json" || { check "rental_end: cozy login again failed: $(tail -c 300 "$OUT/rental-login-2.json")"; return; }
k2=$(jq -r '.device_key // empty' "$OUT/rental-login-2.json" || true)
code=0; cozy machine end "$rental" --force -y > "$OUT/rental-end.txt" 2>&1 || code=$?
[ "$code" = 0 ] && grep -qE 'shut down\.' "$OUT/rental-end.txt" || check "machine end, signed in: exit $code, not shut down: $(tail -c 400 "$OUT/rental-end.txt")"
! cozy --json machine ls 2> /dev/null | J --arg r "$rental" '.machines | any(.rental_id == $r)' || check "machine end, signed in: machine ls still lists $rental"
# Its container goes with it.
gone=''
for _ in $(seq 90); do
  [ -z "$(docker ps -aq --filter label=com.cozy.tensorhub.provider=docker)" ] && { gone=1; break; }
  sleep 1
done
[ -n "$gone" ] || { check "machine end: the Hub still runs its container: $(docker ps -a --filter label=com.cozy.tensorhub.provider=docker --format '{{.Names}} {{.Status}}')"; docker ps -aq --filter label=com.cozy.tensorhub.provider=docker | xargs -r docker rm -f > /dev/null; }

step "logout forgets a key installed from a file, and revokes none another computer uses"
# The 11-11 copy, done again: this computer's key installed from its file (as on a pod), then signed out there.
cp "$key" "$held/laptop-key-2.json"
cozy --json login --key "$held/laptop-key-2.json" > "$OUT/rental-install.json" 2> "$OUT/rental-install.err" || check "login --key with a copy of this computer's key failed: $(tail -c 300 "$OUT/rental-install.json")"
cozy --json logout > "$OUT/rental-logout-installed.json" 2> "$OUT/rental-logout-installed.err" && J '.status == "signed out" and any(.notes[]?; test("not revoked"))' "$OUT/rental-logout-installed.json" \
  || check "logout of an installed key: not forgotten without revoking: $(tail -c 300 "$OUT/rental-logout-installed.json")"
# The computer it was copied from is still signed in.
cozy --json login --key "$held/laptop-key-2.json" > /dev/null 2>&1 || true
cozy --json whoami > "$OUT/rental-whoami-kept.json" 2> "$OUT/rental-whoami-kept.err" && J --arg k "$k2" '.logged_in == true and .device_key == $k' "$OUT/rental-whoami-kept.json" \
  || check "the copied key was revoked by another computer's logout: $(tail -c 300 "$OUT/rental-whoami-kept.json")"

step "a pod's own key: login --export, then login --key there"
signin --export "$held/pod-key.json" "$E2E_HUB_EMAIL" > "$OUT/rental-export.json" || check "login --export failed: $(tail -c 300 "$OUT/rental-export.json")"
pod=$(jq -r '.device_key // empty' "$OUT/rental-export.json" 2> /dev/null || true)
[ -n "$pod" ] && [ "$pod" != "$k2" ] && [ "$pod" != "$k1" ] || check "login --export: no key of its own (${pod:-none}, this computer's $k2)"
cozy --json login --key "$held/pod-key.json" > /dev/null 2>&1 && cozy --json whoami 2> /dev/null | J --arg k "$pod" '.logged_in == true and .device_key == $k' \
  || check "login --key with the exported key: not signed in as it"
cozy --json logout > /dev/null 2>&1 || true
cozy --json login --key "$held/laptop-key-2.json" > /dev/null 2>&1 && cozy --json whoami 2> /dev/null | J --arg k "$k2" '.logged_in == true and .device_key == $k' \
  || check "the pod's logout touched this computer's key"
# Signed out, as the suite was.
cozy --json logout > /dev/null 2>&1 || true
rm -rf "$held"
}

# pod_reach <container>: the pod reaches the isolated Hub at its own origin (loopback) through forward.py, inside the
# container and on Docker's bridge. The bridge one is $FORWARD; killing it keeps the pod from its Hub.
FORWARD=''
pod_reach() {
  local bridge port=${COZY_TEST_HUB_ORIGIN##*:}
  bridge=$(docker network inspect bridge --format '{{(index .IPAM.Config 0).Gateway}}')
  if [ -z "$FORWARD" ] || ! kill -0 "$FORWARD" 2> /dev/null; then
    python3 -I "$here/forward.py" "$bridge" "$port" 127.0.0.1 "$port" > "$OUT/forward-bridge.log" 2>&1 & FORWARD=$!
  fi
  docker cp "$here/forward.py" "$1:/tmp/forward.py" > /dev/null \
    && docker exec -d "$1" python3 -I /tmp/forward.py 127.0.0.1 "$port" "$bridge" "$port"
}

# pod_line <file>: when, and what Calcifer and the pod itself say of the rental: a line of the timeline.
pod_line() {
  local shown address probe
  shown=$(cozy --json machine show "$m" 2> /dev/null | jq -c '{state, phase: .status.phase, deadline: .status.idle_deadline_ms, unreachable, end_failed}' 2> /dev/null || echo '{}')
  address=$(cozy --json machine show "$m" 2> /dev/null | jq -r '.address // empty' 2> /dev/null || true)
  probe=$([ -z "$address" ] || curl -sk --max-time 3 -o /dev/null -w '%{http_code}' "https://$address/v1/status" || true)
  echo "$(date -u +%H:%M:%S.%3N) calcifer=$shown pod_identity_http=${probe:-none}" >> "$1"
}

pod_release() {
step "a pod that releases itself ends on its Hub's word (cl-875, cl-880)"
[ -n "${E2E_HUB_LOG:-}" ] && [ -n "${E2E_HUB_EMAIL:-}" ] && [ -n "${COZY_TEST_HUB_ORIGIN:-}" ] \
  || { check "pod_release: e2e/isolated.sh named no Hub log, email or origin"; return; }
local m='' pid container='' deadline='' code label=label=com.cozy.tensorhub.provider=docker timeline=$OUT/release-timeline.txt
signin "$E2E_HUB_EMAIL" > "$OUT/release-login.json" || { check "pod_release: cozy login failed: $(tail -c 300 "$OUT/release-login.json")"; return; }
# A pod that boots and serves (the worker image the Hub pins), reaching its Hub once its container starts.
cozy --json machine rent cpu -y > "$OUT/release-rent.ndjson" 2> "$OUT/release-rent.err" & pid=$!
for _ in $(seq 1200); do
  container=$(docker ps -q --filter "$label" | head -1)
  [ -z "$container" ] && kill -0 "$pid" 2> /dev/null || break
  sleep 0.5
done
[ -n "$container" ] && pod_reach "$container" || check "pod_release: no container to reach its Hub from"
for _ in $(seq 600); do kill -0 "$pid" 2> /dev/null || break; sleep 1; done
kill "$pid" 2> /dev/null && check "pod_release: no ready pod in 10 minutes: $(tail -c 300 "$OUT/release-rent.ndjson")"
wait "$pid" 2> /dev/null || true
m=$(jq -r 'select(.machine?) | .machine.name // empty' "$OUT/release-rent.ndjson" 2> /dev/null | tail -1 || true)
[ -n "$m" ] && [ -n "$container" ] || { check "pod_release: machine rent cpu: no ready pod: $(tail -c 400 "$OUT/release-rent.ndjson") $(tail -c 300 "$OUT/release-rent.err")"; kill "$FORWARD" 2> /dev/null || true; cozy --json logout > /dev/null 2>&1 || true; return; }
echo "rented $m (container $container)"

# Its idle window made 120 s (`[machine] idle_seconds`, the tests' knob), counted from a keepalive now: the pod restarts
# on it, and this computer follows it to wherever its Hub says it now serves.
docker exec "$container" sh -c 'f=${HOME:-/root}/.cozy/config.toml; mkdir -p "${f%/*}"; touch "$f"
  if grep -q "^\[machine\]" "$f"; then sed -i "/^\[machine\]/a idle_seconds = 120" "$f"; else printf "\n[machine]\nidle_seconds = 120\n" >> "$f"; fi' \
  || check "pod_release: could not set the pod's idle window"
cozy --json machine keepalive "$m" > "$OUT/release-keepalive.json" 2>&1 || check "pod_release: machine keepalive $m failed: $(tail -c 300 "$OUT/release-keepalive.json")"
docker restart "$container" > /dev/null && pod_reach "$container" || check "pod_release: docker restart failed"
for _ in $(seq 120); do
  cozy --json machine show "$m" > "$OUT/release-show.json" 2> /dev/null || true
  deadline=$(jq -r 'select(.unreachable == null and .status.phase == "ready") | .status.idle_deadline_ms // empty' "$OUT/release-show.json" 2> /dev/null || true)
  [ -n "$deadline" ] && [ $(( deadline - $(date +%s%3N) )) -lt 150000 ] && break
  deadline=''; sleep 1
done
[ -n "$deadline" ] || { check "pod_release: $m was not followed back, ready on its 120 s window: $(head -c 400 "$OUT/release-show.json")"; kill "$FORWARD" 2> /dev/null || true; return; }

step "the pod releases itself while kept from its Hub, and this computer is signed out"
cozy --json logout > /dev/null 2>&1 || check "pod_release: logout failed"
local wait_s=$(( (deadline - $(date +%s%3N)) / 1000 - 5 ))
[ "$wait_s" -le 0 ] || sleep "$wait_s"
# Kept from its Hub as its window ends: its release never lands, and its Hub bills on.
pod_line "$timeline"
kill "$FORWARD" 2> /dev/null || true; wait "$FORWARD" 2> /dev/null || true; FORWARD=''
echo "$(date -u +%H:%M:%S.%3N) the pod is kept from its Hub" >> "$timeline"
for _ in $(seq 90); do
  pod_line "$timeline"
  cozy --json machine show "$m" > "$OUT/release-show-released.json" 2> /dev/null || true
  J '.state == "ended" or (.end_failed | length > 0)' "$OUT/release-show-released.json" 2> /dev/null && break
  sleep 1
done
cat "$timeline"
J '.state != "ended" and (.end_failed | test("not_logged_in"))' "$OUT/release-show-released.json" \
  || check "a pod that said it released itself: not shown as unconfirmed and possibly billing: $(head -c 400 "$OUT/release-show-released.json")"
[ -n "$(docker ps -aq --filter "$label")" ] || check "pod_release: its container went: its Hub heard the release after all"
cozy machine ls > "$OUT/release-ls.txt" 2>&1 || true
grep -q "$m was not shut down and may still be billing" "$OUT/release-ls.txt" || check "machine ls does not say $m may still bill: $(tail -c 300 "$OUT/release-ls.txt")"
docker logs "$container" > "$OUT/release-pod.log" 2>&1 || true

step "signed in, its end releases it at its Hub"
signin "$E2E_HUB_EMAIL" > "$OUT/release-login-2.json" || { check "pod_release: cozy login again failed: $(tail -c 300 "$OUT/release-login-2.json")"; return; }
code=0; cozy machine end "$m" --force -y > "$OUT/release-end.txt" 2>&1 || code=$?
[ "$code" = 0 ] && grep -qE 'shut down\.' "$OUT/release-end.txt" || check "machine end $m, signed in: exit $code, not shut down: $(tail -c 400 "$OUT/release-end.txt")"
for _ in $(seq 90); do [ -z "$(docker ps -aq --filter "$label")" ] && break; sleep 1; done
[ -z "$(docker ps -aq --filter "$label")" ] || { check "pod_release: its Hub still runs its container after its end"; docker ps -aq --filter "$label" | xargs -r docker rm -f > /dev/null; }
cozy --json logout > /dev/null 2>&1 || true
}
