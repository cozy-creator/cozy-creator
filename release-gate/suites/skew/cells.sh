# Suite `skew` (hubgate.sh --suite skew --swap <tensorhub binary>): before a Hub deploy, this computer's cozy (the new
# client) against the candidate Hub on the laptop's 8819, whose object store is R2 itself, so every key is signed as
# production's store checks it. Through `cozy` in the default home, on $M (local); no rental, since no pod reaches it:
#   paths     hub_contract against the candidate
#   account   whoami, balance, the rental catalog
#   package   lock, publish 0.1.0 and 0.1.1, then install, run, yank, un-yank and a yanked candidate on $M (hub_packages)
#   model     hub_models on $M: the machine uploads under its run capability (the workload writer, as pods do), then
#             publish, download, the exact pin and lane, delete → gone
# Any 5xx fails the cell. The previous minor's released cozy × the candidate Hub is e2e/skew's, in CI: a second cozy
# here would need a second daemon. Real pods are the `hub` suite's, on production right after the deploy.
HB=$OUT/suite-skew
if [ -z "${HUB_URL:-}" ] || [ "$HUB_URL" = https://tensorhub.com ]; then
  expect "skew hub" FAIL "HUB_URL names no candidate Hub ('${HUB_URL:-}'): run hubgate.sh --suite skew --swap <tensorhub binary>"
  return 0
fi
. "$HERE/suites/hub/lib.sh"
trap 'hub_cleanup; end_it' EXIT

hub_contract
hub_account
hub_do catalog ok machine rent
[ -n "$WHO" ] || return 0
hub_publish
hub_packages
hub_models
hub_cleanup
