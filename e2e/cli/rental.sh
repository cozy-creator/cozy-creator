# e2e/cli/rental.sh: a rental on the isolated Hub's docker provider (a container on this runner, no real pod; it never
# gets ready, as this Hub's ingress is on loopback), and sign-in by the code that development Hub logs instead of
# emailing (e2e/isolated.sh exports its log and email). Linux.
#   rental_end   cl-875: `machine end` says a rental is shut down only once its Hub affirms it ended. Signed out, or
#                holding a key the Hub revoked (11-11: another computer's `logout` revoked a copied key), it exits non-zero
#                naming why, and the rental is still there; signed in again, it ends it. `rent --ssh` with no path takes
#                the default key, and `machine ssh` says what stands in the way: not ready yet, or signed out (not "not
#                a rental"). `logout` revokes a key this computer enrolled, never one installed with `login --key`,
#                which another computer may hold; a pod signs in with a key of its own (`login --export`).

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

step "a rental says what it rents until its Hub names its host, then the host's word (cl-888, Paul 11-12)"
# One host, one name: the Hub names a pod by its host once it's placed there, and until then the rental is "Renting a CPU…",
# never its `rent-…` handle, on `machine ls` (and the page, which reads the same record). Its word is the Hub's own record
# of it (`machine ls --history`).
said=$(jq -r 'select(.type? == "rental.accepted") | .placeholder // empty' "$OUT/rental-rent.ndjson" 2> /dev/null | tail -1 || true)
[ "$said" = "Renting a CPU…" ] || check "rental_end: accepted, it says '${said:-nothing}', not what it rents: $(grep rental.accepted "$OUT/rental-rent.ndjson" | tail -c 300)"
shown='' word='' handles=''
for _ in $(seq 120); do
  cozy machine ls > "$OUT/rental-ls.txt" 2> /dev/null || true
  cozy --json machine ls > "$OUT/rental-ls.json" 2> /dev/null || true
  ! grep -q 'rent-[0-9a-f]' "$OUT/rental-ls.txt" || handles=$(grep 'rent-' "$OUT/rental-ls.txt" | head -2)
  now=$(jq -r --arg r "$rental" '.machines[] | select(.rental_id == $r) | .placeholder // .name' "$OUT/rental-ls.json" 2> /dev/null || true)
  # The JSON keeps its handle as its name, which its placeholder stands in for until its Hub names the host.
  [[ $now != rent-* ]] || handles="machine ls --json: $now with no placeholder"
  [[ " $shown " == *" ${now// /_} "* ]] || shown+=" ${now// /_}"
  word=$(jq -r --arg r "$rental" '.machines[] | select(.rental_id == $r and .placeholder == null) | .name' "$OUT/rental-ls.json" 2> /dev/null || true)
  [ -n "$word" ] && grep -qE "^ *$word " "$OUT/rental-ls.txt" && break
  sleep 0.5
done
echo "rental_end: $rental was shown as:$shown"
[ -z "$handles" ] || check "rental_end: machine ls showed a rent- handle: $handles"
[ -n "$word" ] && [[ $word != rent-* ]] || check "rental_end: its Hub never named $rental's host in a minute; shown as:$shown"
hub=$(cozy --json machine ls --history 2> /dev/null | jq -r --arg r "$rental" '.rentals[] | select(.rental_id == $r) | .name' || true)
[ -n "$word" ] && [ "$hub" = "$word" ] || check "rental_end: machine ls calls $rental '${word:-nothing}', its Hub records '${hub:-nothing}'"

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
