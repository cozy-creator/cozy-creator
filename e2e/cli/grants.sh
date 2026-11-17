# grant_silent (cl-1189). Paul, 11-15: "runs never stuck": a run is failed only on evidence of a hang, never for slowness; a download
# that moves no bytes for 43 minutes is a hang, and the product notices it and recovers: retry, another source, a clear one-line error.
# cr-323's pod-session 44139328775 (chunhwa, 11-17) sat 42 minutes at "choosing the model for lone_probe.models.vae_encoder", its model
# named by hash, until it was cancelled by hand: its capability granted a read the one TensorD held didn't, so its reads waited for a
# redemption the renewal asked again forever, saying nothing. Here the isolated Hub's ingress takes those redemptions and answers nothing
# (COZY_TEST_HUB_HOLD, e2e/skew/ingress.go): e2e/final-model 0.2.0-rc.1, installed on local by its checkpoint's hash, says what it waits
# on with each attempt, and once they run out reads anonymously (it's public) and lands, within GRANT_S.
GRANT_S=240

# grant_held: how many redemptions the ingress has held so far.
grant_held() { local n; n=$(grep -c '"held":true' "${COZY_TEST_HUB_REQUEST_LOG:-/dev/null}" 2> /dev/null) || true; echo "${n:-0}"; }

# grant_events <ndjson>: what the run said it waited on, one line each, in order.
grant_events() {
  grep '^{' "$1" 2> /dev/null | jq -r 'select(.type? == "run.progress" or .type? == "run.waiting") | (.progress.stage? // .reason? // empty)' 2> /dev/null | uniq
}

grant_silent() {
step "a run whose Hub never answers its capability's redemption says each attempt, then reads anonymously (cl-1189)"
[ -n "$LOCAL" ] || return 0
[ "${RUNNER_OS:-}" = Linux ] || return 0
[ -n "${COZY_TEST_HUB_HOLD:-}" ] || { check "grant silent: e2e/skew/run.sh named no ingress hold (COZY_TEST_HUB_HOLD)"; return; }
[ -n "${E2E_HUB_LOG:-}" ] && [ -n "${E2E_HUB_EMAIL:-}" ] || { check "grant silent: e2e/isolated.sh named no Hub log or email to sign in with"; return; }
local dir=$OUT/grant-silent model=e2e/final-model commit began took held pid status signed
rm -rf "$dir"; mkdir -p "$dir"
# Signed in, as a run's reads carry a capability only then; signed out again after, when it found it so.
signed=$(cozy --json whoami 2> /dev/null | jq -r '.account // empty' 2> /dev/null || true)
[ -n "$signed" ] || signin "$E2E_HUB_EMAIL" > "$dir/login.json" || { check "grant silent: cozy login failed: $(tail -c 300 "$dir/login.json")"; return; }
cozy --json model show "$model@0.2.0-rc.1" > "$dir/show.json" 2>&1
commit=$(jq -r '.lanes[0].checkpoint // empty' "$dir/show.json" 2> /dev/null)
if [ -z "$commit" ]; then
  check "grant silent: model show $model@0.2.0-rc.1 named no checkpoint: $(head -c 300 "$dir/show.json")"
  [ -n "$signed" ] || cozy --json logout > /dev/null 2>&1
  return
fi
held=$(grant_held)
touch "$COZY_TEST_HUB_HOLD"
began=$SECONDS
run_submit "$dir" grant model install "$model@$commit" --on local > /dev/null
pid=$(cat "$dir/grant.pid" 2> /dev/null || true)
until run_ended "$dir/grant.ndjson" || ! run_alive "${pid:-0}" || [ $((SECONDS - began)) -ge "$GRANT_S" ]; do sleep 1; done
took=$((SECONDS - began))
rm -f "$COZY_TEST_HUB_HOLD"
held=$(( $(grant_held) - held ))
grant_events "$dir/grant.ndjson" > "$dir/said.txt"
echo "grant silent: the ingress held $held redemptions; in $took s the run said:"; sed 's/^/  /' "$dir/said.txt" | cut -c1-240
if ! run_ended "$dir/grant.ndjson"; then
  check "grant silent: $model@$commit still '$(tail -n 1 "$dir/said.txt" | cut -c1-200)' after $took s while its Hub answered no redemption: no attempt said, nothing landed (cl-1189)"
  # Answered again, the renewal's next ask lands it; a run still going then is cancelled, so the shard goes on.
  local waited=$SECONDS
  until run_ended "$dir/grant.ndjson" || ! run_alive "${pid:-0}" || [ $((SECONDS - waited)) -ge 180 ]; do sleep 1; done
  run_ended "$dir/grant.ndjson" || { cozy --json run cancel "$(cat "$dir/grant.id" 2> /dev/null)" > "$dir/cancel.json" 2>&1 || true; }
fi
# Its record: followed to its end when it said one, else read as it stands, its follower ended.
if run_ended "$dir/grant.ndjson"; then
  run_wait "$dir" grant || true
else
  kill "${pid:-0}" 2> /dev/null || true
  run_show "$dir" grant "$(cat "$dir/grant.number" 2> /dev/null)"
fi
status=$(run_get "$dir" grant .status)
cozy machine logs local --lines 0 --since 1h > "$dir/tensord.log" 2>&1 || true
grep -E "redeem|capability" "$dir/tensord.log" | tail -n 12 | cut -c1-300 || true
[ "$held" -ge 1 ] || check "grant silent: the ingress held no redemption: the run read nothing under a capability, so this proves nothing"
if [ "$took" -lt "$GRANT_S" ]; then
  [ "$status" = completed ] || check "grant silent: $model@$commit ended $status: $(run_get "$dir" grant '.reason.message' | head -c 300)"
  grep -qE 'attempt [0-9]+/[0-9]+' "$dir/said.txt" || check "grant silent: the run never said an attempt while its Hub answered no redemption"
fi
[ -n "$signed" ] || cozy --json logout > "$dir/logout.json" 2>&1 || check "grant silent: logout: $(tail -c 300 "$dir/logout.json")"
}
