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

# going <machine> [cli]: the machine's runs (every project's) and its jobs (an install, a download, an update: cl-1028, no project's
# runs, listed on the machine), as one `{runs}`; fails when neither answers.
going() {
  local runs jobs
  runs=$(timeout 60 "${2:-cozy}" --json run ls --all --on "$1" < /dev/null 2> /dev/null) || runs=
  jobs=$(timeout 60 "${2:-cozy}" --json machine show "$1" < /dev/null 2> /dev/null) || jobs=
  [ -n "$runs$jobs" ] || return 1
  jq -nc --argjson r "${runs:-null}" --argjson j "${jobs:-null}" '{runs: (($r.runs // []) + ($j.jobs // []))}'
}

# run_stands <machine> [cli]: where each running run says it stands, a line each: number, stage, steps done of steps, and
# for a download its bytes. A bare clock moves on any event, so a run can crawl for an hour inside one stage (41904557331).
run_stands() {
  going "$1" "${2:-cozy}" | jq -er '
    .runs[] | select(.status == "in_progress") | (.progress.stage // "starting") as $s
    | "\(.number) \($s) \(.progress.completed // 0)/\(.progress.total // 0)\(if ($s | test("download")) then " \(.progress.bytes_done // 0)" else "" end)"'
}

# run_downloads <machine> [cli]: each running run's download still under way, a line each: number, bytes so far, of bytes.
run_downloads() {
  going "$1" "${2:-cozy}" | jq -r '.runs[] | select(.status == "in_progress")
    | select((.progress.bytes_total // 0) > (.progress.bytes_done // 0)) | "\(.number) \(.progress.bytes_done // 0) \(.progress.bytes_total)"'
}

# run_clock <machine> [cli]: that machine's runs by their own clock: the ms of their newest move (`.progress.at_ms`, which
# stands still on a silent run; a run with none yet by its start or end), then what names each not settled from any folder:
# a run's KEY-N, a job's id (cl-1028). Fails when
# no listing answers.
run_clock() {
  going "$1" "${2:-cozy}" | jq -er '
    ([.runs[] | .progress.at_ms, .settled_at_ms, .started_at_ms, .accepted_at_ms, .created_at_ms | numbers] | max // 0) as $at
    | [$at, (.runs[] | select(.status == "queued" or .status == "in_progress") | if (.name // "") != "" then .name else .id end)] | map(tostring) | join(" ")'
}

# The pod side of `pod_derived`, found once as this file is sourced: a session cds into its packages and outputs, and a
# path relative to where it started found nothing after its first arm (44034679486).
POD_DERIVED_SCRIPT=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/derived-state.sh
# pod_derived <cli> <machine> <reset|census>: release-gate/derived-state.sh on the machine's root shell (an `--ssh` rental):
# `reset` clears what earlier runs left besides weights and installs (LoRA bakes, memos, learned peaks, kernels, page cache);
# both print its census line, `derived N memo N peaks N kernels N jit N cached_gib X`. Nothing printed: no answer. No
# script here: exit 2 at once, as no retry finds it.
pod_derived() {
  local script
  [ -f "$POD_DERIVED_SCRIPT" ] || { echo "no $POD_DERIVED_SCRIPT" >&2; return 2; }
  script=$(base64 -w0 < "$POD_DERIVED_SCRIPT") || return 2
  timeout 300 "$1" machine ssh "$2" -- "echo $script | base64 -d > /var/tmp/derived-state.sh && sh /var/tmp/derived-state.sh $3" < /dev/null |
    grep -E '^derived [0-9]+ memo [0-9]+ peaks [0-9]+ kernels [0-9]+ jit [0-9]+' | tail -1
}
# pod_derived_zero <census line>: no derived page, memo entry or learned peak (an arm's own executors may compile kernels as
# they boot).
pod_derived_zero() { awk '{exit !($2 == 0 && $4 == 0 && $6 == 0)}' <<<"$1"; }
