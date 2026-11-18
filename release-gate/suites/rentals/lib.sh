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
    elif $f == "offline" then (.offline // "")
    else (.[$f] // "") end'
}
# gone <rental id>: its Hub released it and proved it absent at its provider (`machine ls --history`: released).
gone() {
  "$CLI" --json machine ls --history 2>/dev/null | jq -e --arg r "$1" '.rentals[] | select(.rental_id == $r and .state == "released")' >/dev/null
}

# rental_idle_through_updates <machine> <rental id>: an idle rental that gets an update every 3 minutes (the RC follower's,
# hougetsu's 23:27, 23:37, 23:49) still releases itself 15 minutes after its last use and is destroyed. Its last use is a
# keepalive now; the updates name the Runtime it runs, so each is a whole update with nothing to install. Red before
# cl-1203: each update restarted the idle clock.
rental_idle_through_updates() {
  local m=$1 r=$2 runtime d0 now moved=0
  "$CLI" machine keepalive "$m" >"$RT/keepalive.out" 2>&1
  # Its Status carries the new deadline a moment later.
  for _ in 1 2 3 4 5 6; do
    d0=$(show "$m" deadline); (( ${d0:-0} >= $(date +%s%3N) + 840000 )) && break; sleep 5
  done
  runtime=$(show "$m" runtime)
  if [ -z "$d0" ] || [ "$d0" = 0 ] || [ -z "$runtime" ]; then
    expect "rental-idle-through-updates" FAIL "$m reports no idle deadline or Runtime after a keepalive: $(tail -c 200 "$RT/keepalive.out")"
    return
  fi
  log "rental-idle-through-updates: $m idle until $(date -u -d "@$((d0 / 1000))" +%T), updates every 3 min"
  while now=$(date +%s%3N); (( now < d0 + 180000 )); do
    if (( now < d0 - 30000 )); then
      "$CLI" --json machine update "$m" --runtime "$runtime" -y >>"$RT/updates.out" 2>&1
      local d; d=$(show "$m" deadline)
      [ -z "$d" ] || [ "$d" = 0 ] || [ "$d" = "$d0" ] || { moved=$d; log "rental-idle-through-updates: an update moved $m's deadline to $(date -u -d "@$((d / 1000))" +%T)"; }
    fi
    gone "$r" && break
    sleep 180
  done
  for _ in $(seq 12); do gone "$r" && break; sleep 10; done
  if gone "$r"; then
    expect "rental-idle-through-updates" PASS "$m released itself at its idle deadline $(date -u -d "@$((d0 / 1000))" +%T) through updates every 3 min, and its Hub proved it gone"
  else
    expect "rental-idle-through-updates" FAIL "$m not released 5 min past its idle deadline $(date -u -d "@$((d0 / 1000))" +%T)${moved:+; the updates moved it to $(date -u -d "@$((moved / 1000))" +%T)}: state $(show "$m" state)"
    "$CLI" machine end "$m" -y >>"$RT/end.out" 2>&1
  fi
}

# rental_stop_closes [provider]: a rental whose provider stops its pod is ending at once and destroyed, never "offline": its
# container's PID 1 ends (`cozy machine ssh`), which vast reads as `exited`, as its balance stop of hougetsu did. The Hub
# confirms the stop with a second read and releases it (th-404); Calcifer says `ending: vast stopped it`. RunPod starts an
# exited container again, so only vast stops it this way. Red before th-404: offline until 15 minutes after its last reach.
rental_stop_closes() {
  local p=${1:-vast} m r t0 st seen=() offline= ending=
  [ -s "$RT/pod-ssh" ] || ssh-keygen -q -t ed25519 -N '' -C rentals-stop -f "$RT/pod-ssh"
  # vast without `cpu` now (as suite hub found on 11-10): its cheapest GPUs in turn.
  local sku
  for sku in cpu ${RENTALS_GPU_FALLBACK:-rtx-5060-ti rtx-3070 rtx-3060 rtx-4060-ti rtx-3080}; do
    "$CLI" --json machine rent "$sku" --provider "$p" --ssh "$RT/pod-ssh.pub" -y >"$RT/rent.out" 2>&1
    grep -q '"rental.sku_out_of_stock"' "$RT/rent.out" || break
    log "rental-stop-closes: no $sku on $p now; the next"
  done
  r=$(grep -m1 '"type":"rental.accepted"' "$RT/rent.out" | jq -r '.rental_id // empty' 2>/dev/null)
  m=$(grep -o '"machine":{"name":"[^"]*"' "$RT/rent.out" | head -1 | sed 's/.*"name":"//; s/"$//')
  if [ -z "$m" ] || [ -z "$r" ]; then
    expect "rental-stop-closes" FAIL "no $p rental ($sku): $(tail -c 300 "$RT/rent.out")"
    [ -z "$r" ] || "$CLI" machine end "$r" -y >>"$RT/end.out" 2>&1
    return
  fi
  local agent SSH_AUTH_SOCK SSH_AGENT_PID; agent=$(ssh-agent -s) && eval "$agent" >/dev/null && ssh-add -q "$RT/pod-ssh"
  for _ in 1 2 3 4 5 6; do "$CLI" machine ssh "$m" -- 'kill -TERM 1' >"$RT/kill.out" 2>&1 && break; sleep 10; done
  ssh-agent -k >/dev/null 2>&1
  t0=$(date +%s)
  log "rental-stop-closes: $m ($r, $p): its PID 1 ended at $(date -u -d "@$t0" +%T)"
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
