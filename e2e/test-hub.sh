# Synthetic mutations must use the Hub started by the isolated fixture runner.
TEST_HUB_GUARD=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/test_hub.py
require_test_hub() {
  python3 "$TEST_HUB_GUARD" --hub "$1" --expected-origin "${COZY_TEST_HUB_ORIGIN:-}"
}

# Manual operator commands remain explicit; CI verification always needs its own Hub.
require_ci_test_hub() {
  [ -n "${CI:-}${GITHUB_ACTIONS:-}" ] || return 0
  require_test_hub "${1:-${COZY_HUB_URL:-${HUB_URL:-}}}"
}

# require_ci_rental_hub prod|isolated: a CI run that rents real machines (release-gate.yml's gates, pod-session.yaml) names its
# Hub. `prod` is tensorhub.com, which alone has providers: real pods rent there (Paul, 11-11: two Hubs, production and the
# laptop's). `isolated` is a verified test Hub. Synthetic publication never takes this path: it always needs require_test_hub.
require_ci_rental_hub() {
  [ -n "${CI:-}${GITHUB_ACTIONS:-}" ] || return 0
  case ${1:-} in
    prod)
      [ "${COZY_HUB_URL:-https://tensorhub.com}" = https://tensorhub.com ] \
        || { echo "hub=prod rents on https://tensorhub.com, not ${COZY_HUB_URL}" >&2; return 2; }
      echo "this run rents real machines on production, https://tensorhub.com: its dispatch named hub=prod" >&2 ;;
    isolated) require_ci_test_hub ;;
    *) echo "a CI run that rents real machines names its Hub: hub=prod or hub=isolated" >&2; return 2 ;;
  esac
}
