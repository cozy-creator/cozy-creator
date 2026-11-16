# dev_start (cl-1069): a dev home's Calcifer, built as scripts/dev-local.sh builds it (the dev profile, CARGO_PROFILE_DEV_DEBUG=0), at
# the stack size it ships. Paul's laptop crash-looped on 2026-11-16 ("thread 'tokio-rt-worker' has overflowed its stack" 1-10 s after
# each start, 08:05-08:09Z): a render walk's poll nests large futures, about 9.4 MiB deep in a dev build, past tokio's 2 MiB default.
# Here: a render walk going on `local`, Calcifer started again so it resumes the walk at its start, and a `/fit` estimate; Calcifer
# stays up 60 s with no restart of its own, `cozy doctor` answers, the walk's take lands, and its log says no overflow.
# E2E_DEV_COZY: the change's cozy built so (e2e.yaml's dev-start shard).
devstart_here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

dev_start() {
[ -n "$LOCAL" ] || return 0
step "a dev build's Calcifer resumes a render walk at its start and answers a /fit estimate, and stays up (cl-1069)"
[ -x "${E2E_DEV_COZY:-}" ] || { check "dev start: no dev-profile cozy was built (E2E_DEV_COZY)"; return; }
local t0 back=$OUT/devstart-candidate restarts n drop=$HOME/.config/systemd/user/cozy-calcifer.service.d/devstart-stderr.conf
t0=$(date +%s)
cp "$COZY" "$back"
# Calcifer's stderr, where an overflow says so, goes nowhere by default: here, to its journal.
mkdir -p "${drop%/*}" && printf '[Service]\nStandardError=journal\n' > "$drop" && systemctl --user daemon-reload
CAND_COZY=$E2E_DEV_COZY install_candidate_binary
cozy restart > "$OUT/devstart-restart.txt" 2>&1 || check "dev start: Calcifer didn't start on the dev build: $(tail -c 300 "$OUT/devstart-restart.txt")"
echo "dev start: Calcifer on the dev build, $(cozy --version)"
run_page add a --tool "$devstart_here/project/waits.py/wait" --fields '{"instructions": "A lighthouse at dusk, its lamp just lit.", "seconds": 45}' \
  > "$OUT/devstart-add.json" 2>&1 || check "dev start: the box wasn't made: $(tail -c 300 "$OUT/devstart-add.json")"
run_page render --on local --queue a > "$OUT/devstart-render.json" 2>&1 || check "dev start: the render wasn't asked: $(tail -c 300 "$OUT/devstart-render.json")"
# Its walk sends it: its job runs on local, so the next start has a walk to resume.
for _ in $(seq 120); do
  run_page board > "$OUT/devstart-board.json" 2> /dev/null &&
    jq -e '.jobs[]? | select(.asset == "a" and (.state == "running" or .state == "waiting") and .run)' "$OUT/devstart-board.json" > /dev/null && break
  sleep 5
done
jq -e '.jobs[]? | select(.asset == "a" and .run)' "$OUT/devstart-board.json" > /dev/null 2>&1 || check "dev start: the walk sent nothing in 10 min: $(head -c 400 "$OUT/devstart-board.json")"
restarts=$(systemctl --user show -p NRestarts --value cozy-calcifer.service)
cozy restart > "$OUT/devstart-restart.txt" 2>&1 || check "dev start: Calcifer didn't start again: $(tail -c 300 "$OUT/devstart-restart.txt")"
run_page fit a > "$OUT/devstart-fit.json" 2>&1 || check "dev start: /fit wasn't answered: $(tail -c 300 "$OUT/devstart-fit.json")"
sleep 60
cozy --json doctor > "$OUT/devstart-doctor.json" 2>&1
jq -e . "$OUT/devstart-doctor.json" > /dev/null 2>&1 || check "dev start: cozy doctor didn't answer: $(tail -c 300 "$OUT/devstart-doctor.json")"
n=$(systemctl --user show -p NRestarts --value cozy-calcifer.service)
[ "$n" = "$restarts" ] || check "dev start: Calcifer started again by itself $((n - restarts)) times in 60 s"
# The resumed walk lands its take.
for _ in $(seq 120); do
  [ "$(run_page asset a 2> /dev/null | jq '.versions | length' 2> /dev/null)" -ge 1 ] 2> /dev/null && break
  sleep 5
done
[ "$(run_page asset a 2> /dev/null | jq '.versions | length' 2> /dev/null)" -ge 1 ] 2> /dev/null || check "dev start: the resumed walk landed no take of a in 10 min"
journalctl --user -u cozy-calcifer.service --since "@$t0" --no-pager > "$OUT/devstart-journal.log" 2>&1
! grep -q "overflowed its stack" "$OUT/devstart-journal.log" ||
  check "dev start: Calcifer overflowed its stack $(grep -c 'overflowed its stack' "$OUT/devstart-journal.log") times: $(grep -m1 'overflowed its stack' "$OUT/devstart-journal.log")"
# The candidate again, for what follows.
rm -f "$drop"; systemctl --user daemon-reload
CAND_COZY=$back install_candidate_binary
cozy restart > /dev/null 2>&1 || true
}
