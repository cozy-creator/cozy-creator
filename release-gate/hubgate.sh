#!/bin/bash
# hubgate.sh --suite skew|hub [--swap TENSORHUB_BIN] [--hub URL] [--on MACHINE] [--out DIR]: the Hub's suites, which need
# no GPU and no baseline, through this computer's `cozy` in its default home (release-gate README, "Clients and the Hub").
#   skew   before a Hub deploy: --swap puts the candidate binary on this laptop's 8819 Hub for the run, under the local
#          stack's lock and logged in its log; 8819's database is dumped first and restored after, its binary put back.
#   hub    after a Hub deploy, before any client is promoted: production, with its own rentals (real pods).
# --on is where package and model cells run (default: local). PASS when cells ran and none FAILed (expect.tsv).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
SUITE= SWAP= HUB_URL=${HUB_URL:-} ON= OUT=
while [ $# -gt 0 ]; do
  case $1 in
    --suite) SUITE=$2;; --swap) SWAP=$(realpath "$2");; --hub) HUB_URL=$2;; --on) ON=$2;; --out) OUT=$2;;
    *) echo "unknown argument $1" >&2; exit 2;;
  esac; shift 2
done
[ -f "$HERE/suites/$SUITE/cells.sh" ] || { echo "hubgate.sh needs --suite skew or hub" >&2; exit 2; }
OUT=$(realpath -m "${OUT:-$PWD/hubgate-$SUITE-$(date -u +%Y%m%dT%H%MZ)}"); mkdir -p "$OUT"; cd "$OUT" || exit 2
: > expect.tsv
log() { echo "$(date -u +%H:%M:%SZ) $*" | tee -a gate.log; }
expect() { printf '%s\t%s\t%s\n' "$1" "$2" "$(tr '\n\t' '  ' <<< "$3" | cut -c1-400)" >> expect.tsv; log "$1: $2 $3"; }
end_it() { :; }  # the suites end their own rentals
CLI=cozy M=${ON:-local} R=$(( $(date +%s%N) / 1000 % 4000000000 ))
. "$HERE/../e2e/run.sh"

# The laptop's 8819 Hub (systemd user unit tensorhub-8819, R2-backed): swapped only under the stack's lock (laptop-live-dev-stack).
T=$HOME/cozy_v2/agent-tools H=$HOME/.local/share/cozy/local-hub UNIT=$HOME/.config/systemd/user/tensorhub-8819.service.d
DROP=$UNIT/999-zskew.conf LOCAL=http://127.0.0.1:8819 SWAPPED= DUMPED=
stack() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) hubgate skew (xs-166): $*" >> "$T/local-stack.log"; log "stack: $*"; }
q() { psql "$1" -v ON_ERROR_STOP=1 -Atq -c "$2"; }
healthy() {
  local code=000
  for _ in $(seq 150); do
    code=$(curl -s -o /dev/null -w '%{http_code}' "$LOCAL/healthz" || true)
    [ "$code" = 204 ] && break
    systemctl --user is-failed -q tensorhub-8819 && break
    sleep 2
  done
  echo "$code"
}
counts() { q "$DB_URL" "SELECT (SELECT count(*) FROM hub_v2.repos)||' repos '||(SELECT count(*) FROM hub_v2.orgs)||' orgs '||(SELECT count(*) FROM hub_v2.model_commits)||' commits'"; }
execpath() { systemctl --user show tensorhub-8819 -p ExecStart --value | sed -n 's/.*path=\([^ ;]*\).*/\1/p'; }
restore() {
  [ -n "$DUMPED" ] || return 0
  local db=${DB_URL##*/} admin=${DB_URL%/*}/postgres code=1 why=
  db=${db%%\?*}
  systemctl --user stop tensorhub-8819
  if [ -n "$SWAPPED" ]; then
    q "$admin" "DROP DATABASE IF EXISTS skew_restore" >/dev/null 2>&1
    if ! q "$admin" "CREATE DATABASE skew_restore TEMPLATE template0 ENCODING 'UTF8' LC_COLLATE 'en_US.utf8' LC_CTYPE 'en_US.utf8' OWNER tensorhub" >> restore.txt 2>&1; then why=create
    elif ! nice -n 19 pg_restore --exit-on-error -d "${DB_URL%/*}/skew_restore" 8819.dump >> restore.txt 2>&1; then why=pg_restore
    else
      q "$admin" "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname IN ('$db','skew_restore') AND pid<>pg_backend_pid()" >/dev/null 2>&1
      if q "$admin" "ALTER DATABASE $db RENAME TO ${db}_skewpost" >> restore.txt 2>&1; then
        if q "$admin" "ALTER DATABASE skew_restore RENAME TO $db" >> restore.txt 2>&1; then
          q "$DB_URL" "SELECT billing.arm_book('Paul Fidika: 8819 restored from its own dump after hubgate skew (xs-166)', now())" >> restore.txt 2>&1
          [ "$(q "$DB_URL" "SELECT billing.book_armed()")" = t ] && code=0 || why='book not armed'
        else why=rename; q "$admin" "ALTER DATABASE ${db}_skewpost RENAME TO $db" >> restore.txt 2>&1; fi
      else why=rename; fi
    fi
    rm -f "$DROP"; systemctl --user daemon-reload
  else code=0; fi
  systemctl --user start tensorhub-8819
  local h after; h=$(healthy); after=$(counts)
  [ $code = 0 ] && [ "$after" = "$BEFORE" ] && [ -n "$SWAPPED" ] && q "$admin" "DROP DATABASE ${db}_skewpost" >> restore.txt 2>&1
  stack "8819 restored (exit $code${why:+ $why}), back on $(execpath), healthz $h, counts as before: $([ "$after" = "$BEFORE" ] && echo yes || echo "no ($after; before $BEFORE)"); lock released"
}
if [ -n "$SWAP" ]; then
  HUB_URL=${HUB_URL:-$LOCAL}
  [ "$HUB_URL" = "$LOCAL" ] || { echo "--swap puts the candidate on $LOCAL; --hub $HUB_URL names another" >&2; exit 2; }
  DB_URL=$(grep -o 'TENSORHUB_DATABASE_URL=.*' "$H/secrets.env" | cut -d= -f2-)
  exec 7> "$T/.local-stack.lock"; flock -w 900 7 || { echo "the local stack's lock stayed held 15 min" >&2; exit 2; }
  cozy() { command cozy "$@" 7>&-; }  # nothing started here keeps the lock
  trap restore EXIT
  live=$(q "$DB_URL" "SELECT count(*) FROM hub_v2.provider_resources WHERE state<>'absent'")
  [ "$live" = 0 ] || { log "8819 holds $live live provider resources: nothing swapped"; exit 1; }
  BEFORE=$(counts)
  systemctl --user stop tensorhub-8819; DUMPED=1
  nice -n 19 pg_dump -Fc -d "$DB_URL" -f 8819.dump > dump.txt 2>&1 || { log "pg_dump failed: $(tail -2 dump.txt)"; exit 1; }
  printf '[Service]\nExecStart=\nExecStart=%s --config %s/config.local.yaml --dotenv %s/secrets.env serve\n' "$SWAP" "$H" "$H" > "$DROP"
  SWAPPED=1; systemctl --user daemon-reload; systemctl --user start tensorhub-8819
  stack "start (lock held to its end): 8819 stopped, DB dumped ($(pg_restore -l 8819.dump | grep -c ' TABLE ') TABLE entries; $BEFORE), then $("$SWAP" version) by $(basename "$DROP"); cozy here: $(command cozy --version)"
  h=$(healthy); [ "$h" = 204 ] || { log "the candidate on 8819 did not come up (healthz $h): $(journalctl --user -u tensorhub-8819 -n 20 --no-pager 2>&1 | tail -c 1500)"; exit 1; }
fi
HUB_URL=${HUB_URL:-https://tensorhub.com}
log "hubgate $SUITE: Hub $HUB_URL$([ -n "$SWAP" ] && echo " ($("$SWAP" version))"), cozy $(command cozy --version), on $M, seeds from $R"
run_project "$OUT/project" || { log "cozy project new failed"; exit 1; }
( . "$HERE/suites/$SUITE/cells.sh" )
fails=$(grep -c $'\tFAIL\t' expect.tsv) passes=$(grep -c $'\tPASS\t' expect.tsv)
grep $'\tFAIL\t' expect.tsv | cut -f1,3 | sed 's/^/FAIL /' | tee verdict.txt
if [ "$fails" = 0 ] && [ "$passes" -gt 0 ]; then echo "VERDICT PASS ($passes cells)" | tee -a verdict.txt
else echo "VERDICT FAIL ($fails of $((fails + passes)) cells)" | tee -a verdict.txt; exit 1; fi
