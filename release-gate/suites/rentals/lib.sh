# release-gate/suites/rentals/lib.sh: a rental's life from end to end (cl-1203, th-404), on real pods. Sourced by cells.sh under
# gate.sh (which gives expect, log, $CLI, $M, $OUT), or by run.sh by hand. Paul, 11-17, of hougetsu (2× H100 NVL, $14.04/h):
# "why is it listed as offline? why didn't it end itself from idle timer?"; "Either the rental is alive, or closed (or closing)";
# "make sure we don't waste money like that again".
RT=${RT:-$OUT/rentals}; mkdir -p "$RT"
# show <machine> <field>: `cozy machine show --json`'s state, idle deadline (unix ms), reason, offline or runtime.
show() {
  "$CLI" --json machine show "$1" 2>/dev/null | jq -r --arg f "$2" '
    if $f == "deadline" then (.status.idle_deadline_ms // 0)
    elif $f == "runtime" then (.status.runtime // "")
    elif $f == "version" then (.status.version // "")
    elif $f == "offline" then (.offline // "")
    else (.[$f] // "") end'
}
# settled <machine>: no update or other job of Calcifer's queued or going on it (up to 5 min).
settled() {
  for _ in $(seq 30); do
    "$CLI" --json machine show "$1" 2>/dev/null | jq -e '[.jobs[]? | select(.status == "queued" or .status == "in_progress")] | length == 0' >/dev/null && return
    sleep 10
  done
}
# put_tensord <machine> <bin>: the candidate TensorD on a pod of the cell's own, after the RC follower's move at its boot (on
# mucheng, 11-18, one queued before the candidate replaced it after); false unless the machine runs it then.
put_tensord() {
  local want; want=$("$2" --version | awk '{print $2}')
  for _ in 1 2; do
    settled "$1"
    "$CLI" machine update "$1" --tensord "$2" -y >>"$RT/tensord-$1.out" 2>&1
    settled "$1"
    [ "$(show "$1" version)" = "$want" ] && return
  done
  return 1
}
# gone <rental id>: its Hub released it and proved it absent at its provider (`machine ls --history`: released).
gone() {
  "$CLI" --json machine ls --history 2>/dev/null | jq -e --arg r "$1" '.rentals[] | select(.rental_id == $r and .state == "released")' >/dev/null
}

# rental_idle_through_updates <machine> <rental id>: an idle rental that gets an update every 3 minutes (the RC follower's,
# hougetsu's 23:27, 23:37, 23:49) still releases itself 15 minutes after its last use and is destroyed. Its last use is a
# keepalive now; each update sets the mode it has (`--mode dedicated`), which drains and restarts its service as an RC
# update does, whatever the open RC's builds are. Red before cl-1203: each update restarted the idle clock.
rental_idle_through_updates() {
  local m=$1 r=$2 d0 now moved= updated=0
  "$CLI" machine keepalive "$m" >"$RT/keepalive.out" 2>&1
  # Its Status carries the new deadline a moment later.
  for _ in 1 2 3 4 5 6; do
    d0=$(show "$m" deadline); (( ${d0:-0} >= $(date +%s%3N) + 840000 )) && break; sleep 5
  done
  if [ -z "$d0" ] || [ "$d0" = 0 ]; then
    expect "rental-idle-through-updates" FAIL "$m reports no idle deadline after a keepalive: $(tail -c 200 "$RT/keepalive.out")"
    return
  fi
  log "rental-idle-through-updates: $m idle until $(date -u -d "@$((d0 / 1000))" +%T), updates every 3 min"
  while now=$(date +%s%3N); (( now < d0 + 180000 )); do
    if (( now < d0 - 30000 )); then
      if "$CLI" --json machine update "$m" --mode dedicated -y >>"$RT/updates.out" 2>&1; then updated=$((updated + 1)); else
        log "rental-idle-through-updates: an update failed: $(tail -c 200 "$RT/updates.out")"; fi
      local d; d=$(show "$m" deadline)
      [ -z "$d" ] || [ "$d" = 0 ] || [ "$d" = "$d0" ] || { moved=$d; log "rental-idle-through-updates: an update moved $m's deadline to $(date -u -d "@$((d / 1000))" +%T)"; }
    fi
    gone "$r" && break
    sleep 180
  done
  for _ in $(seq 12); do gone "$r" && break; sleep 10; done
  if (( updated < 3 )); then
    expect "rental-idle-through-updates" FAIL "only $updated of its updates went through, so the cell proves nothing: $(tail -c 200 "$RT/updates.out")"
    gone "$r" || "$CLI" machine end "$m" -y >>"$RT/end.out" 2>&1
  elif gone "$r"; then
    expect "rental-idle-through-updates" PASS "$m released itself at its idle deadline $(date -u -d "@$((d0 / 1000))" +%T) through $updated updates, and its Hub proved it gone"
  else
    expect "rental-idle-through-updates" FAIL "$m not released 5 min past its idle deadline $(date -u -d "@$((d0 / 1000))" +%T)${moved:+; the updates moved it to $(date -u -d "@$((moved / 1000))" +%T)}: state $(show "$m" state)"
    "$CLI" machine end "$m" -y >>"$RT/end.out" 2>&1
  fi
}

# rental_stop_closes [provider]: a rental whose provider stops its pod is ending at once and destroyed, never "offline". The
# pod stops itself at its provider, as vast stopped hougetsu for its balance: a dev machine (`dev = true` in its config.toml)
# whose service takes SIGUSR2 asks its provider to stop it with the credential only it holds (`stop_when_asked`; an exited
# container is started again by vast and RunPod alike). The Hub confirms the stop with a second read and releases it
# (th-404); Calcifer says `ending: vast stopped it`. Red before th-404: offline until 15 minutes after its last reach. A
# candidate TensorD ($TDF) goes on the pod first.
rental_stop_closes() {
  local p=${1:-vast} m r t0 st seen=() offline= ending=
  [ -s "$RT/pod-ssh" ] || ssh-keygen -q -t ed25519 -N '' -C rentals-stop -f "$RT/pod-ssh"
  # vast without `cpu` now (as suite hub found on 11-10): its cheapest GPUs in turn.
  local sku
  # An order that is not up in 7 minutes (waiting for capacity at the price it agreed, th-403) is ended, and the next asked.
  for sku in cpu ${RENTALS_GPU_FALLBACK:-rtx-5060-ti rtx-3070 rtx-3060 rtx-4060-ti rtx-3080}; do
    timeout 420 "$CLI" --json machine rent "$sku" --provider "$p" --disk-gb 50 --ssh "$RT/pod-ssh.pub" -y >"$RT/rent.out" 2>&1
    grep -q '"machine":{"name"' "$RT/rent.out" && break
    r=$(grep -m1 '"type":"rental.accepted"' "$RT/rent.out" | jq -r '.rental_id // empty' 2>/dev/null)
    [ -z "$r" ] || "$CLI" machine end "$r" -y >>"$RT/end.out" 2>&1
    log "rental-stop-closes: no $sku up on $p now; the next"
  done
  r=$(grep -m1 '"type":"rental.accepted"' "$RT/rent.out" | jq -r '.rental_id // empty' 2>/dev/null)
  m=$(grep -o '"machine":{"name":"[^"]*"' "$RT/rent.out" | head -1 | sed 's/.*"name":"//; s/"$//')
  if [ -z "$m" ] || [ -z "$r" ]; then
    expect "rental-stop-closes" FAIL "no $p rental ($sku): $(tail -c 300 "$RT/rent.out")"
    [ -z "$r" ] || "$CLI" machine end "$r" -y >>"$RT/end.out" 2>&1
    return
  fi
  if [ -n "${TDF:-}" ] && ! put_tensord "$m" "$TDF"; then
    expect "rental-stop-closes" FAIL "$m does not run the candidate TensorD: $(show "$m" version); $(tail -c 200 "$RT/tensord-$m.out")"
    "$CLI" machine end "$m" -y >>"$RT/end.out" 2>&1
    return
  fi
  local agent SSH_AUTH_SOCK SSH_AGENT_PID; agent=$(ssh-agent -s) && eval "$agent" >/dev/null && ssh-add -q "$RT/pod-ssh"
  local ask="mkdir -p ~/.cozy && touch ~/.cozy/config.toml && { echo 'dev = true'; grep -v '^dev *=' ~/.cozy/config.toml || true; } > ~/.cozy/config.toml.new && mv ~/.cozy/config.toml.new ~/.cozy/config.toml && pkill -USR2 -P 1 -x tensord && echo asked"
  for _ in 1 2 3 4 5 6; do "$CLI" machine ssh "$m" -- "$ask" </dev/null >"$RT/stop.out" 2>&1 && grep -q asked "$RT/stop.out" && break; sleep 10; done
  ssh-agent -k >/dev/null 2>&1
  t0=$(date +%s)
  if ! grep -q asked "$RT/stop.out"; then
    expect "rental-stop-closes" FAIL "could not ask $m to stop at $p: $(tail -c 200 "$RT/stop.out")"
    "$CLI" machine end "$m" -y >>"$RT/end.out" 2>&1
    return
  fi
  log "rental-stop-closes: $m ($r, $p): asked to stop itself at its provider at $(date -u -d "@$t0" +%T)"
  while (( $(date +%s) - t0 < 600 )); do
    st=$(show "$m" state); [ -z "$(show "$m" offline)" ] || offline=$(show "$m" offline)
    [ "${seen[-1]:-}" = "$st" ] || seen+=("$st")
    { [ "$st" = ending ] || [ "$st" = ended ]; } && [ -z "$ending" ] && ending=$(( $(date +%s) - t0 ))
    gone "$r" && break
    sleep 15
  done
  local said="states ${seen[*]}"
  if [ -n "$offline" ]; then
    expect "rental-stop-closes" FAIL "$m was shown offline ($offline); $said"
  elif ! gone "$r"; then
    expect "rental-stop-closes" FAIL "$m not released and gone 10 min after its provider stopped it; $said"
  elif [ -z "$ending" ] || (( ending > 300 )); then
    expect "rental-stop-closes" FAIL "$m was ending only ${ending:-never} s after its provider stopped it; $said"
  else
    expect "rental-stop-closes" PASS "$m ending ${ending} s after its provider stopped it, released and gone at $p; $said"
  fi
  gone "$r" || "$CLI" machine end "$m" -y >>"$RT/end.out" 2>&1
}
