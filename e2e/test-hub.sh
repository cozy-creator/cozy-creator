# Synthetic mutations must use the Hub started by the isolated fixture runner.
TEST_HUB_GUARD=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/test_hub.py
require_test_hub() {
  python3 "$TEST_HUB_GUARD" --hub "$1" --expected-origin "${COZY_TEST_HUB_ORIGIN:-}"
}
