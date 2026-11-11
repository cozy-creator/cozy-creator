# Suite `hub` (gate.sh --suite hub, or hubgate.sh --suite hub --on local): production right after a Hub deploy, before
# any client is promoted (release-gate README, "Clients and the Hub"). The API contract as cozy uses it, on the machine
# $M (the gate's rental, or this computer's), and the real-pod cells on fresh rentals of its own; a throwaway
# <account>/ci-hub-<R> package and models that the suite retires at the end (the releases yanked, the models deleted):
#   paths     hub_contract: each path no verb reaches answers as its contract says, through e2e/hubcheck
#   account   whoami (the account, its balance, the buy page), the rental catalog, the rental $M itself (gate.sh's)
#   package   lock and publish 0.1.0 and 0.1.1 (hub_publish); after the pods, hub_packages on $M
#   pods      on each provider (RunPod, vast; `--provider` is dev-only, turned on by `dev = true`): a `cpu` rental with a
#             throwaway SSH key, the package run there, new weights uploaded from the pod (the workload writer), no Hub
#             secret in any process's environment and the machine authenticated by its own key (xs-157), ended and
#             gone (hub_pod). A cozy that refuses `--provider` fails both, and the cells run once on the Hub's choice.
#   model     hub_models on $M: upload, publish, download, the exact pin and lane, delete → gone
# Each is an `expect` row: a FAIL fails the gate, and so does any 5xx from the Hub. The migration rehearsal on a prod
# dump is the release agent's step before a Hub deploy (release-gate README).
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
hub_publish
hub_dev
refused=
for p in ${HUB_PROVIDERS:-runpod vast}; do hub_pod "$p" || [ $? != 2 ] || refused=1; done
[ -z "$refused" ] || hub_pod any
hub_packages
hub_models
hub_cleanup
