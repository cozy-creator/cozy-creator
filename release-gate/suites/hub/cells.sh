# Production read/provider checks after a Hub deploy. Synthetic repository writes run in the companion
# isolated fixture job (e2e/skew/run.sh), never through this production account.
# The provider path still rents, runs a weightless local package by content, checks credentials/auth,
# and ends each owned rental. No package or model repository is created here.
HB=$OUT/suite-hub HUB_URL=${HUB_URL:-https://tensorhub.com}
. "$HERE/suites/hub/lib.sh"
trap 'hub_cleanup; declare -F tfs_cleanup >/dev/null && tfs_cleanup; end_it' EXIT

hub_contract
hub_account
hub_do catalog ok machine rent
if [ -z "${ON:-}" ]; then  # the rental itself is gate.sh's (rent, then end and check it is gone)
  P=$("$CLI" --json machine show "$M" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["rental"]["sku"])' 2>/dev/null)
  [ -n "$P" ] && expect "hub rental" PASS "$M is a $P rental" || expect "hub rental" FAIL "machine show $M names no rental"
fi
[ -n "$WHO" ] || return 0
hub_dev
refused=
for p in ${HUB_PROVIDERS:-runpod vast}; do hub_pod "$p" || [ $? != 2 ] || refused=1; done
[ -z "$refused" ] || hub_pod any
hub_cleanup
