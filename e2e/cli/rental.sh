# e2e/cli/rental.sh: a rental on the isolated Hub's docker provider (a container on this runner, no real pod; it never
# gets ready, as this Hub's ingress is on loopback), and sign-in by the code that development Hub logs instead of
# emailing (e2e/isolated.sh exports its log and email). Linux.
#   rental_end   cl-875: `machine end` says a rental is shut down only once its Hub affirms it ended. Signed out, or
#                holding a key the Hub revoked (11-11: another computer's `logout` revoked a copied key), it exits non-zero
#                naming why, and the rental is still there; signed in again, it ends it. `rent --ssh` with no path takes
#                the default key, and `machine ssh` says what stands in the way: not ready yet, or signed out (not "not
#                a rental"). `logout` revokes a key this computer enrolled, never one installed with `login --key`,
#                which another computer may hold; a pod signs in with a key of its own (`login --export`).
#   rental_end_again  cl-977: an end that failed (signed out) is asked again by Calcifer itself, paced: signed in again, the
#                rental ends with no second `machine end`, and its container goes.

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
# cl-1076, Paul 11-16: "300GB worker discs by default". A rent that names no disk asks its Hub for 300 GB: the quote it
# made first, as hubcheck keeps it (this Hub's cpu product would be bought at its image's own 512 GB either way).
grep -F '"operation":"POST /v1/rental-quotes"' "$OUT/hub-kept.jsonl" 2> /dev/null | jq -se 'last.body.container_disk_gb == 300' > /dev/null \
  || check "rental_end: machine rent cpu didn't ask for 300 GB: $(grep -F rental-quotes "$OUT/hub-kept.jsonl" 2> /dev/null | tail -1 | head -c 300)"

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
end_refused key-refused 'sign in to the Hub again'

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
cozy --json logout > "$OUT/rental-logout-installed.json" 2> "$OUT/rental-logout-installed.err" && J '.status == "signed out" and any(.notes[]?; test("didn.t revoke it"))' "$OUT/rental-logout-installed.json" \
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

rental_end_again() {
step "an end that failed is asked again by Calcifer itself (cl-977)"
[ -n "${E2E_HUB_LOG:-}" ] && [ -n "${E2E_HUB_EMAIL:-}" ] || { check "rental_end_again: e2e/isolated.sh named no Hub log or email to sign in with"; return; }
local pid rental code at ended=''
signin "$E2E_HUB_EMAIL" > "$OUT/again-login.json" || { check "rental_end_again: cozy login failed: $(tail -c 300 "$OUT/again-login.json")"; return; }
cozy --json machine rent cpu -y > "$OUT/again-rent.ndjson" 2> "$OUT/again-rent.err" & pid=$!
for _ in $(seq 180); do
  rental=$(jq -r 'select(.type? == "rental.accepted") | .rental_id // empty' "$OUT/again-rent.ndjson" 2> /dev/null | tail -1 || true)
  [ -n "$rental" ] || ! kill -0 "$pid" 2> /dev/null || { sleep 1; continue; }
  break
done
kill -KILL "$pid" 2> /dev/null || true
wait "$pid" 2> /dev/null || true
[ -n "$rental" ] || { check "rental_end_again: machine rent cpu: no rental accepted: $(tail -c 400 "$OUT/again-rent.ndjson")"; cozy --json logout > /dev/null 2>&1 || true; return; }
cozy --json logout > /dev/null 2>&1 || check "rental_end_again: logout failed"
code=0; cozy --json machine end "$rental" --force -y > "$OUT/again-end.json" 2> "$OUT/again-end.err" || code=$?
[ "$code" != 0 ] && J '.error.code == "end_failed"' "$OUT/again-end.json" || check "rental_end_again: machine end signed out: exit $code, not end_failed: $(tail -c 300 "$OUT/again-end.json")"
signin "$E2E_HUB_EMAIL" > "$OUT/again-login-2.json" || { check "rental_end_again: cozy login again failed: $(tail -c 300 "$OUT/again-login-2.json")"; return; }
# Asked again a minute after it failed (then two, four...): no `machine end` from here.
at=$(date +%s)
for _ in $(seq 240); do
  cozy --json machine show "$rental" > "$OUT/again-show.json" 2> /dev/null || true
  J '.state == "ended"' "$OUT/again-show.json" && { ended=1; break; }
  sleep 1
done
[ -n "$ended" ] || check "rental_end_again: Calcifer never asked its end again: $(jq -c '{state, end_failed, end_unconfirmed}' "$OUT/again-show.json" 2> /dev/null) $(($(date +%s) - at)) s after sign-in"
for _ in $(seq 90); do [ -z "$(docker ps -aq --filter label=com.cozy.tensorhub.provider=docker)" ] && break; sleep 1; done
[ -z "$(docker ps -aq --filter label=com.cozy.tensorhub.provider=docker)" ] \
  || { check "rental_end_again: the Hub still runs its container"; docker ps -aq --filter label=com.cozy.tensorhub.provider=docker | xargs -r docker rm -f > /dev/null; }
[ -z "$ended" ] || echo "rental_end_again: $rental ended by Calcifer's own second ask, $(($(date +%s) - at)) s after sign-in"
cozy --json logout > /dev/null 2>&1 || true
}

# run_on_new (cl-1066, Paul's fix-don't-refuse): a project's `cozy run --on new:cpu -y` rents for it, quoted and agreed. Before, its rent
# was refused `project_remote_execution_unavailable` ("A project's render can't rent a machine by itself") and the run failed: every run
# is a project's since cl-1028, so `--on new` rented for none. This Hub's docker rental never gets ready, so where the run stands is the
# cell: on its new rental, waiting for it, not failed.
run_on_new() {
step "a project's run --on new rents for it (cl-1066)"
[ -n "${E2E_HUB_LOG:-}" ] && [ -n "${E2E_HUB_EMAIL:-}" ] && [ -n "${E2E_FINAL:-}" ] || { check "run_on_new: e2e/isolated.sh named no Hub log, email or e2e-final"; return; }
local P=$OUT/on-new-project pid run='' w=$OUT/on-new-watch.json rental=''
signin "$E2E_HUB_EMAIL" > "$OUT/on-new-login.json" || { check "run_on_new: cozy login failed: $(tail -c 300 "$OUT/on-new-login.json")"; return; }
cozy --json new --unencrypted "$P" > "$OUT/on-new-new.json" 2>&1 || check "run_on_new: new: $(tail -c 300 "$OUT/on-new-new.json")"
(cd "$P" && cozy --json -y run "$E2E_FINAL/echo" text=rented --on new:cpu) > "$OUT/on-new-run.ndjson" 2> "$OUT/on-new-run.err" & pid=$!
for _ in $(seq 120); do
  in_project "$P" --json run ls --limit 5 > "$OUT/on-new-runs.json" 2> /dev/null || true
  run=$(jq -r '[.runs[]? | select(.target | test("e2e-final"))] | min_by(.number) | .number // empty' "$OUT/on-new-runs.json" 2> /dev/null || true)
  if [ -n "$run" ]; then
    in_project "$P" --json run watch "$run" --once > "$w" 2>&1 || true
    rental=$(cozy --json machine ls 2> /dev/null | jq -r --arg m "$(jq -r '.machine // empty' "$w")" '.machines[] | select(.name == $m and .rental_id != null) | .rental_id' 2> /dev/null | head -1 || true)
    [ -z "$rental" ] || J '.status == "failed"' "$w" || break
    J '.status == "failed"' "$w" && break
  fi
  sleep 2
done
kill -KILL "$pid" 2> /dev/null || true
wait "$pid" 2> /dev/null || true
echo "run_on_new: run ${run:-none}: $(jq -c '{status, machine, waiting, reason: .reason.code}' "$w" 2> /dev/null), rental ${rental:-none}"
[ -n "$run" ] || check "run_on_new: run --on new:cpu made no run: $(tail -c 300 "$OUT/on-new-run.ndjson") $(tail -c 300 "$OUT/on-new-run.err")"
[ -n "$rental" ] && ! J '.status == "failed"' "$w" \
  || check "run_on_new: the project's run --on new:cpu rented nothing for it: $(jq -c '{status, machine, reason}' "$w" 2> /dev/null)"
[ -z "$rental" ] || cozy --json machine end "$rental" --force -y > "$OUT/on-new-end.json" 2>&1 || check "run_on_new: its rental didn't end: $(tail -c 300 "$OUT/on-new-end.json")"
on_new_encrypted
cozy --json logout > /dev/null 2>&1 || true
}
# on_new_encrypted: the same from an encrypted project; once it locks, Calcifer's own store holds nothing that ties the rental to it (no
# traces, cl-984): its rent asks name neither the run, the project's id nor its code (coordinator, 11-17).
on_new_encrypted() {
local E=$OUT/on-new-encrypted pass=$OUT/on-new-pass pid number='' id='' code='' runid='' rental='' w=$OUT/on-new-enc-watch.json
printf 'on-new-%s%s\n' "$RANDOM" "$RANDOM" > "$pass"
# stderr apart: `cozy new` says "Creating encrypted project: …" there, and jq reads the JSON alone.
cozy --json new "$E" --passphrase-file "$pass" --recovery-out "$OUT/on-new-recovery" > "$OUT/on-new-enc-new.json" 2> "$OUT/on-new-enc-new.err" \
  || { check "run_on_new: new encrypted: $(tail -c 300 "$OUT/on-new-enc-new.err")"; return; }
id=$(jq -r '.id // empty' "$OUT/on-new-enc-new.json") code=$(jq -r '.code // empty' "$OUT/on-new-enc-new.json")
(cd "$E" && cozy --json -y run "$E2E_FINAL/echo" text=sealed --on new:cpu) > "$OUT/on-new-enc-run.ndjson" 2> "$OUT/on-new-enc-run.err" & pid=$!
for _ in $(seq 120); do
  number=$(in_project "$E" --json run ls --limit 5 2> /dev/null | jq -r '[.runs[]? | select(.target | test("e2e-final"))] | min_by(.number) | .number // empty' 2> /dev/null || true)
  if [ -n "$number" ]; then
    in_project "$E" --json run watch "$number" --once > "$w" 2> "$w.err" || true
    rental=$(cozy --json machine ls 2> /dev/null | jq -r --arg m "$(jq -r '.machine // empty' "$w")" '.machines[] | select(.name == $m and .rental_id != null) | .rental_id' 2> /dev/null | head -1 || true)
    [ -z "$rental" ] && ! J '.status == "failed"' "$w" || break
  fi
  sleep 2
done
kill -KILL "$pid" 2> /dev/null || true
wait "$pid" 2> /dev/null || true
runid=$(jq -r '.id // empty' "$w" 2> /dev/null || true)
[ -n "$rental" ] && ! J '.status == "failed"' "$w" || check "run_on_new: the encrypted project's run --on new:cpu rented nothing for it: $(jq -c '{status, machine, reason}' "$w" 2> /dev/null)"
[ -z "$rental" ] || cozy --json machine end "$rental" --force -y > "$OUT/on-new-enc-end.json" 2>&1 || check "run_on_new: the encrypted project's rental didn't end"
in_project "$E" --json vault lock > "$OUT/on-new-enc-lock.json" 2>&1 || check "run_on_new: the encrypted project didn't lock: $(tail -c 300 "$OUT/on-new-enc-lock.json")"
python3 - "$HOME/.cozy/calcifer/state.db" "$id" "$code" "$runid" > "$OUT/on-new-traces.txt" 2>&1 <<'PY' || check "run_on_new: once locked, Calcifer's asks tie the rental to the encrypted project: $(cat "$OUT/on-new-traces.txt")"
import sqlite3, sys
path, *marks = sys.argv[1:]
marks = [mark for mark in marks if mark]
assert len(marks) == 3, f"what to look for: {marks}"
db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
rows = db.execute("SELECT key, coalesce(run, ''), body, coalesce(reason, '') FROM asks").fetchall()
found = [(mark[:8], row[0]) for row in rows for mark in marks
         if mark.encode() in b"".join(value if isinstance(value, bytes) else str(value).encode() for value in row)]
assert not found, f"{found} in {len(rows)} asks"
print(f"{len(rows)} asks, none naming the run, the project's id or its code")
PY
}

# rental_rename (cl-1089): a render sent to a rental by its `rent-…` handle, before its Hub names the host, follows the name: cl-1069's
# pod sessions (43849240267, 43850691690) waited on "no machine named rent-…" until cut. Its run says the host's name, never that wait,
# and once the rental ends it settles. This Hub's docker rental never gets ready, so the run is never sent: where it stands is the cell.
# (Since cl-1028 a `cozy run` of local code there makes nothing it can follow: its files wait for the machine, and its install is the
# machine's job. A render's run is made at once.)
# cl-1106 (xs-203's user-flow, pod-sessions 43948824859 and 43875113783: "both set to 'preferred'" read unchecked once named): a project
# that chose the rental under its handle, as the page's Prefer does while it boots, holds it under the Hub's name: `status` (what the
# page reads) says that name, and a render then goes there, not wherever a project with none goes.
rename_here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# rename_order <try>: a cpu rental ordered, its follower killed once accepted (the order stands), as fast as it's accepted: its id and
# handle in $rental and $handle.
rename_order() {
  local pid
  rental='' handle=''
  cozy --json machine rent cpu -y > "$OUT/rename-rent-$1.ndjson" 2> "$OUT/rename-rent-$1.err" & pid=$!
  for _ in $(seq 1800); do
    rental=$(jq -r 'select(.type? == "rental.accepted") | .rental_id // empty' "$OUT/rename-rent-$1.ndjson" 2> /dev/null | tail -1 || true)
    [ -n "$rental" ] || ! kill -0 "$pid" 2> /dev/null || { sleep 0.1; continue; }
    break
  done
  handle=$(jq -r 'select(.type? == "rental.accepted") | .name // empty' "$OUT/rename-rent-$1.ndjson" 2> /dev/null | tail -1 || true)
  kill -KILL "$pid" 2> /dev/null || true
  wait "$pid" 2> /dev/null || true
}
# rename_render <box> <machines, JSON>: the project's box rendered as the page renders it.
rename_render() {
  project_api "$P" /runs -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: rename-$1-$RANDOM$RANDOM" \
    -d "{\"names\":[\"$1\"],\"on\":$2}" > "$OUT/rename-render-$1.json" 2>&1 || check "rental_rename: render $1: $(tail -c 300 "$OUT/rename-render-$1.json")"
}
rental_rename() {
step "a render and a project's chosen machine, a rental named by its handle, follow its Hub's name for the host (cl-1089, cl-1106)"
[ -n "${E2E_HUB_LOG:-}" ] && [ -n "${E2E_HUB_EMAIL:-}" ] || { check "rental_rename: e2e/isolated.sh named no Hub log or email to sign in with"; return; }
local rental='' handle='' name='' P=$OUT/rename-project chose='' try number='' w=$OUT/rename-watch.json
signin "$E2E_HUB_EMAIL" > "$OUT/rename-login.json" || { check "rental_rename: cozy login failed: $(tail -c 300 "$OUT/rename-login.json")"; return; }
cozy --json new --unencrypted "$P" > "$OUT/rename-new.json" 2>&1 || check "rental_rename: new: $(tail -c 300 "$OUT/rename-new.json")"
jq -n --arg wait "$rename_here/project/waits.py/wait" --arg draw "$rename_here/project/draw.py/draw" --arg r "$RANDOM" '[
  {name: "asked", tool: $wait, kind: "image", fields: {instructions: "Sent to the rental by its handle, \($r).", seconds: 1}},
  {name: "preferred", tool: $draw, kind: "image", fields: {instructions: "Where the project prefers, \($r)."}}]' | boxes_in "$P" || true
# Chosen under its handle, as the page's Prefer is seconds after the order, and a render sent to it by that handle, before its Hub
# names the host: a try whose host was named first (the answer says that name) is ended and ordered again.
for try in 1 2 3; do
  rename_order "$try"
  [ -n "$rental" ] && [[ $handle == rent-* ]] || break
  in_project "$P" --json machine use "$handle" > "$OUT/rename-use-$try.json" 2>&1 || true
  J --arg h "$handle" '.machines == [$h]' "$OUT/rename-use-$try.json" && { chose=$handle; break; }
  echo "rental_rename: try $try, $handle was named before it was chosen: $(head -c 300 "$OUT/rename-use-$try.json")"
  cozy --json machine end "$rental" -y > "$OUT/rename-end-$try.json" 2>&1 || true
done
[ -n "$rental" ] && [[ $handle == rent-* ]] || { check "rental_rename: no rental accepted with a handle: $(tail -c 400 "$OUT/rename-rent-$try.ndjson")"; cozy --json logout > /dev/null 2>&1 || true; return; }
[ -n "$chose" ] || check "rental_rename: $handle wasn't chosen under its handle in 3 orders: $(head -c 300 "$OUT/rename-use-$try.json")"
rename_render asked "[\"$handle\"]"
for _ in $(seq 120); do
  name=$(cozy --json machine ls 2> /dev/null | jq -r --arg r "$rental" '.machines[] | select(.rental_id == $r and .placeholder == null) | .name' 2> /dev/null || true)
  [ -n "$name" ] && [[ $name != rent-* ]] && break
  sleep 1
done
if [ -z "$name" ] || [[ $name == rent-* ]]; then
  check "rental_rename: its Hub never named $rental's host in 2 min"
else
  # cl-1089: the render sent to it by its handle. Its local code is locked first: minutes on a cold runner.
  for _ in $(seq 120); do
    number=$(in_project "$P" --json run ls --limit 20 2> /dev/null | jq -r '[.runs[]? | select(.target | test("wait"))] | min_by(.number) | .number // empty' 2> /dev/null || true)
    [ -z "$number" ] || break
    sleep 5
  done
  if [ -z "$number" ]; then
    check "rental_rename: the render sent to $handle made no run: $(status "$P" | jq -c '.board.jobs[]? | select(.asset == "asked")' 2> /dev/null)"
  else
    for _ in $(seq 60); do
      in_project "$P" --json run watch "$number" --once > "$w" 2>&1 || true
      J --arg n "$name" '.machine == $n' "$w" && break
      sleep 2
    done
    echo "rental_rename: run $number, rendered on $handle, after its host was named $name: $(jq -c '{machine, status, waiting}' "$w" 2> /dev/null)"
    J --arg n "$name" '.machine == $n' "$w" || check "rental_rename: the run stayed on $handle after its host was named $name: $(jq -c '{machine, waiting}' "$w" 2> /dev/null)"
    ! J '(.waiting // "") | test("no machine named")' "$w" || check "rental_rename: the run waits on $(jq -r .waiting "$w")"
  fi
  # cl-1106: the project's machines, as the page and `cozy status` read them, and where a render naming none goes: its tool's install
  # (which waits for the rental) or its call there, and its call nowhere else. Reading the project describes its box's tool on `local`,
  # which installs it there too: an install isn't where the render went.
  if [ -n "$chose" ]; then
    status "$P" > "$OUT/rename-status.json" || true
    echo "rental_rename: the project chose $handle; once its host was named $name its machines are $(jq -c '.machines' "$OUT/rename-status.json" 2> /dev/null)"
    J --arg n "$name" '.machines == [$n]' "$OUT/rename-status.json" \
      || check "rental_rename: the project chose $handle, and once its host was named $name its machines are $(jq -c '.machines' "$OUT/rename-status.json" 2> /dev/null), not [$name]"
    rename_render preferred '[]'
    local there='[.runs[]? | select((.target | test("draw")) and .machine == $n)] | length > 0'
    local elsewhere='[.runs[]? | select((.target | test("draw")) and .machine != $n and (.target | test("^(install|sync) ") | not))] | length > 0'
    for _ in $(seq 300); do
      in_project "$P" --json run ls --limit 20 > "$OUT/rename-placed.json" 2> /dev/null || true
      J --arg n "$name" "($there) or ($elsewhere)" "$OUT/rename-placed.json" && break
      sleep 2
    done
    # Its call isn't sent elsewhere meanwhile: `local`'s install for reading it ends in a minute.
    for _ in $(seq 45); do
      J --arg n "$name" "$elsewhere" "$OUT/rename-placed.json" && break
      sleep 2
      in_project "$P" --json run ls --limit 20 > "$OUT/rename-placed.json" 2> /dev/null || true
    done
    echo "rental_rename: the render naming no machine, its project preferring $name: $(jq -c '[.runs[]? | select(.target | test("draw")) | {number, target, machine, status}]' "$OUT/rename-placed.json" 2> /dev/null), its box $(status "$P" | jq -c '{job: (.board.jobs[]? | select(.asset == "preferred") | {state, why}), said: .board.said}' 2> /dev/null)"
    J --arg n "$name" "($there) and (($elsewhere) | not)" "$OUT/rename-placed.json" \
      || check "rental_rename: the render naming no machine went to $(jq -c '[.runs[]? | select(.target | test("draw")) | "\(.target) on \(.machine)"]' "$OUT/rename-placed.json" 2> /dev/null), not $name, the machine its project prefers"
  fi
fi
project_api "$P" /cancel -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: rename-cancel-$RANDOM$RANDOM" -d '{"names":["preferred"]}' > "$OUT/rename-cancel.json" 2>&1 || true
# Ended, the rental settles the run, whichever name it was sent by.
cozy --json machine end "$rental" -y > "$OUT/rename-end.json" 2>&1 || check "rental_rename: machine end $rental: $(tail -c 300 "$OUT/rename-end.json")"
if [ -n "$number" ]; then
  for _ in $(seq 90); do
    in_project "$P" --json run watch "$number" --once > "$w" 2>&1 || true
    J '.status == "failed" or .status == "canceled"' "$w" && break
    sleep 2
  done
  J '.status == "failed" or .status == "canceled"' "$w" || check "rental_rename: the run didn't settle when $rental ended: $(jq -c '{machine, status, waiting}' "$w" 2> /dev/null)"
fi
project_api "$P" /cancel -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: rename-cancel-$RANDOM$RANDOM" -d '{"names":["asked"]}' > "$OUT/rename-cancel-asked.json" 2>&1 || true
cozy --json logout > /dev/null 2>&1 || true
}

# replan_name <rental id>: what machine ls calls the rental once its Hub named its host. replan_quit: the rental ended, signed out.
replan_name() { cozy --json machine ls 2> /dev/null | jq -r --arg r "$1" '.machines[] | select(.rental_id == $r and .placeholder == null) | .name' 2> /dev/null || true; }
replan_quit() { cozy --json machine end "$rental" -y > /dev/null 2>&1 || true; cozy --json logout > /dev/null 2>&1 || true; }
# rental_replan (cl-1125). Paul, 11-17: "I had a job queued on 'project' but that job disappeared or something for some reason. It never
# ran, was just queued forever." "then it gave up and quit? I don't know what happened. Do jobs sometimes expire after a period of time if
# there's no runner for them? or perhaps it was assigned and then the machine died or something? bad experience either way."
# His RTX 5090's image pull stalled, and its Hub replanned the order onto another host, which it named again: rent-24713671, kigurumi,
# then cecilia, with Calcifer restarted between. His render's install, sent to the order and followed to kigurumi (cl-1089), failed
# `machine_lost` ("kigurumi shut down before it took PRO6-1") at the second name, and his take went with it. Work queued for an order
# follows it to whatever host it gets; only the order's end settles it, and that says why in one line.
# This Hub's docker provider has one host, which a replanned attempt is named by again: the second name is its record of the order's
# host renamed in its own database, as a replan's attempt is named. The rental never gets ready here, so the render waits, as Paul's did.
rental_replan() {
step "work queued for a rental order follows it through a Calcifer restart and a second host name; its end says why (cl-1125)"
[ -n "${E2E_HUB_LOG:-}" ] && [ -n "${E2E_HUB_EMAIL:-}" ] || { check "rental_replan: e2e/isolated.sh named no Hub log or email to sign in with"; return; }
local rental='' handle='' first='' second="replan-$RANDOM" P=$OUT/replan-project pg='' number='' w=$OUT/replan-watch.json s=$OUT/replan-status.json
signin "$E2E_HUB_EMAIL" > "$OUT/replan-login.json" || { check "rental_replan: cozy login failed: $(tail -c 300 "$OUT/replan-login.json")"; return; }
cozy --json new --unencrypted "$P" > "$OUT/replan-new.json" 2>&1 || check "rental_replan: new: $(tail -c 300 "$OUT/replan-new.json")"
jq -n --arg wait "$rename_here/project/waits.py/wait" --arg r "$RANDOM" \
  '[{name: "queued", tool: $wait, kind: "image", fields: {instructions: "Queued for the order, \($r).", seconds: 1}}]' | boxes_in "$P" || true
rename_order replan
[ -n "$rental" ] && [[ $handle == rent-* ]] || { check "rental_replan: no rental accepted with a handle: $(tail -c 400 "$OUT/rename-rent-replan.ndjson")"; cozy --json logout > /dev/null 2>&1 || true; return; }
# Preferred and rendered on the order by its handle, as the page does seconds after it.
in_project "$P" --json machine use "$handle" > "$OUT/replan-use.json" 2>&1 || true
project_api "$P" /runs -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: replan-$RANDOM$RANDOM" \
  -d "{\"names\":[\"queued\"],\"on\":[\"$handle\"]}" > "$OUT/replan-render.json" 2>&1 || check "rental_replan: render: $(tail -c 300 "$OUT/replan-render.json")"
for _ in $(seq 120); do first=$(replan_name "$rental"); [ -n "$first" ] && [[ $first != rent-* ]] && break; sleep 1; done
[ -n "$first" ] && [[ $first != rent-* ]] || { check "rental_replan: its Hub never named $rental's host in 2 min"; replan_quit; return; }
# The render's install waits on the order, under the host's first name. Its local code is locked first: minutes on a cold runner.
for _ in $(seq 120); do
  number=$(in_project "$P" --json run ls --limit 20 2> /dev/null | jq -r '[.runs[]? | select(.target | test("wait"))] | min_by(.number) | .number // empty' 2> /dev/null || true)
  [ -z "$number" ] || break
  sleep 5
done
[ -n "$number" ] || { check "rental_replan: the render sent to $handle made no run: $(status "$P" | jq -c '.board.jobs' 2> /dev/null)"; replan_quit; return; }
for _ in $(seq 60); do in_project "$P" --json run watch "$number" --once > "$w" 2>&1 || true; J --arg n "$first" '.machine == $n' "$w" && break; sleep 2; done
echo "rental_replan: run $number waits for $handle, named $first: $(jq -c '{machine, status, waiting}' "$w" 2> /dev/null)"
# Calcifer restarts, as the laptop's did while Paul waited.
systemctl --user restart cozy-calcifer || check "rental_replan: systemctl --user restart cozy-calcifer failed"
for _ in $(seq 60); do cozy --json machine ls > /dev/null 2>&1 && break; sleep 1; done
# Its Hub replans the order onto another host, named anew.
pg=$(docker ps --filter name=cozy-skew-pg --format '{{.Names}}' | head -1)
docker exec "$pg" psql -U postgres -v ON_ERROR_STOP=1 -tAc "UPDATE hub_v2.provider_resources r SET machine_name = '$second'
  FROM hub_v2.rental_acquisition_attempts a WHERE a.rental_id = '$rental' AND a.provider_resource_id = r.id" > "$OUT/replan-hub.txt" 2>&1 \
  && grep -q 'UPDATE [1-9]' "$OUT/replan-hub.txt" || check "rental_replan: the Hub's record of $rental's host wasn't renamed ($pg): $(tail -c 300 "$OUT/replan-hub.txt")"
local now=''
for _ in $(seq 120); do now=$(replan_name "$rental"); [ "$now" = "$second" ] && break; sleep 1; done
[ "$now" = "$second" ] || check "rental_replan: Calcifer still calls $rental '${now:-nothing}' 2 min after its Hub named it $second"
# What was queued for the order follows it there, still queued: never failed while the order stands.
for _ in $(seq 30); do
  in_project "$P" --json run watch "$number" --once > "$w" 2>&1 || true
  J --arg n "$second" '.machine == $n or .status == "failed"' "$w" && break
  sleep 2
done
echo "rental_replan: run $number after its order's host was named $first, then $second: $(jq -c '{machine, status, waiting, reason}' "$w" 2> /dev/null)"
J --arg n "$second" '.machine == $n and .status != "failed" and .status != "canceled"' "$w" \
  || check "rental_replan: run $number, queued for $rental, didn't follow it from $first to $second: $(jq -c '{machine, status, reason}' "$w" 2> /dev/null)"
status "$P" > "$s" || true
echo "rental_replan: the project's machines $(jq -c '.machines' "$s" 2> /dev/null), its take $(jq -c '{job: [.board.jobs[]? | select(.asset == "queued") | {state, error}], said: .board.said}' "$s" 2> /dev/null)"
J --arg n "$second" '.machines == [$n]' "$s" || check "rental_replan: the project preferred $handle; once named $first, then $second, its machines are $(jq -c '.machines' "$s" 2> /dev/null)"
J '[.board.jobs[]? | select(.asset == "queued" and (.state == "waiting" or .state == "running"))] | length == 1' "$s" \
  || check "rental_replan: the take queued for $rental isn't waiting for it: $(jq -c '[.board.jobs[]? | select(.asset == "queued")]' "$s" 2> /dev/null)"
# The order ends: what waited for it settles, saying why in one line, and the take says so too; nothing goes silently.
cozy --json machine end "$rental" -y > "$OUT/replan-end.json" 2>&1 || check "rental_replan: machine end $rental: $(tail -c 300 "$OUT/replan-end.json")"
for _ in $(seq 90); do
  in_project "$P" --json run watch "$number" --once > "$w" 2>&1 || true
  J '.status == "failed" or .status == "canceled"' "$w" && break
  sleep 2
done
echo "rental_replan: run $number once $rental ended: $(jq -c '{machine, status, reason}' "$w" 2> /dev/null)"
J --arg n "$second" '.status == "failed" and (.reason.message | test($n) and (test("\n") | not))' "$w" \
  || check "rental_replan: run $number didn't settle in one line naming $second when its order ended: $(jq -c '{status, reason}' "$w" 2> /dev/null)"
for _ in $(seq 30); do
  status "$P" > "$s" || true
  J --arg n "$second" '[.board.said[]?.message, .board.jobs[]?.error.message?, .board.errors[]?.message] | map(select(type == "string")) | any(test($n))' "$s" && break
  sleep 2
done
echo "rental_replan: the take once $rental ended: $(jq -c '{job: [.board.jobs[]? | select(.asset == "queued") | {state, error}], said: .board.said}' "$s" 2> /dev/null)"
J --arg n "$second" '[.board.said[]?.message, .board.jobs[]?.error.message?, .board.errors[]?.message] | map(select(type == "string")) | any(test($n))' "$s" \
  || check "rental_replan: the render said nothing of $second's end: $(jq -c '.board' "$s" 2> /dev/null | head -c 600)"
project_api "$P" /cancel -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: replan-cancel-$RANDOM$RANDOM" -d '{"names":["queued"]}' > "$OUT/replan-cancel.json" 2>&1 || true
cozy --json logout > /dev/null 2>&1 || true
}
