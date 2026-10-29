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
