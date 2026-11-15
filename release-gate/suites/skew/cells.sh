# Suite `skew`: a client against the isolated candidate Hub started by e2e/skew/run.sh.
# The temporary database and object store are destroyed afterward; no production credentials are used.
#   paths     hub_contract against the candidate (paths-empty: it holds the laptop's data, not production's)
#   account   whoami, balance, the rental catalog
#   package   lock, publish 0.1.0 and 0.1.1, then install, run, yank and un-yank on $M (hub_packages)
#   model     hub_models on $M: the machine uploads under its run capability (the workload writer, as pods do), then
#             publish, download, the exact pin and lane, delete → gone
# Any 5xx fails the cell. Real provider checks remain in the production `hub` suite.
HB=$OUT/suite-skew
if [ -z "${HUB_URL:-}" ] || [ "$HUB_URL" = https://tensorhub.com ]; then
  expect "skew hub" FAIL "HUB_URL names no isolated Hub: use e2e/skew/run.sh"
  return 0
fi
. "$HERE/suites/hub/lib.sh"
hub_test_target || return 2
trap 'hub_cleanup; end_it' EXIT

hub_contract paths-empty
hub_account
hub_do catalog ok machine rent
[ -n "$WHO" ] || return 0
hub_publish
hub_packages
hub_models
hub_cleanup
