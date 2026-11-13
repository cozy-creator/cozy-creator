# release-gate/machines.sh: the machine listing every pod guard reads (gate.sh, sessions/lib.sh, the workflows' cleanup).
# `machine ls` since cozy v0.13.9, `machine list` before it. A listing that does not answer counts every pod as live.

# machines_json [cli]: the listing as JSON; fails when neither verb answers with one.
machines_json() {
  local cli=${1:-cozy} verb j
  for verb in ls list; do
    j=$(timeout 30 "$cli" machine "$verb" --json < /dev/null 2>/dev/null) &&
      jq -e '.machines | type == "array"' <<<"$j" > /dev/null 2>&1 && { printf '%s\n' "$j"; return 0; }
  done
  return 1
}

# listed <rental id or machine name> [cli]: a machine of that rental id or name that has not ended, or no listing.
listed() {
  local j; j=$(machines_json "${2:-cozy}") || return 0
  jq -e --arg r "$1" '.machines[] | select((.rental_id == $r or .name == $r) and .state != "ended")' <<<"$j" > /dev/null
}

# own [cli]: what this computer's Calcifer ordered and has not ended, a line each: its rental id, else (an order its Hub
# has not answered) its name. On a fresh runner that is what this job ordered: another client's rentals read `ordered: false`.
own() { machines_json "${1:-cozy}" | jq -r '.machines[] | select(.kind == "rental" and .ordered and .state != "ended") | .rental_id // .name'; }

# run_stands <machine> [cli]: where each running run says it stands, a line each: number, stage, steps done of steps, and
# for a download its bytes. A bare clock moves on any event, so a run can crawl for an hour inside one stage (41904557331).
run_stands() {
  timeout 60 "${2:-cozy}" --json run ls --all --on "$1" < /dev/null 2> /dev/null | jq -er '
    .runs[] | select(.status == "in_progress") | (.progress.stage // "starting") as $s
    | "\(.number) \($s) \(.progress.completed // 0)/\(.progress.total // 0)\(if ($s | test("download")) then " \(.progress.bytes_done // 0)" else "" end)"'
}

# run_downloads <machine> [cli]: each running run's download still under way, a line each: number, bytes so far, of bytes.
run_downloads() {
  timeout 60 "${2:-cozy}" --json run ls --all --on "$1" < /dev/null 2> /dev/null | jq -r '.runs[] | select(.status == "in_progress")
    | select((.progress.bytes_total // 0) > (.progress.bytes_done // 0)) | "\(.number) \(.progress.bytes_done // 0) \(.progress.bytes_total)"'
}

# run_clock <machine> [cli]: that machine's runs by their own clock: the ms of their newest move (`.progress.at_ms`, which
# stands still on a silent run; a run with none yet by its start or end), then the numbers of those not settled. Fails when
# no listing answers.
run_clock() {
  timeout 60 "${2:-cozy}" --json run ls --all --on "$1" < /dev/null 2> /dev/null | jq -er '
    ([.runs[] | .progress.at_ms, .settled_at_ms, .started_at_ms, .accepted_at_ms, .created_at_ms | numbers] | max // 0) as $at
    | [$at, (.runs[] | select(.status == "queued" or .status == "in_progress") | .number)] | map(tostring) | join(" ")'
}
