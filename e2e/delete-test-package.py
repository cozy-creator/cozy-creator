"""Clean up a release-suite throwaway through the retained administration API."""
import json
import os
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request

from test_hub import require_test_hub


def main() -> int:
    org, name = sys.argv[1].split("/", 1)
    if not name.startswith(("ci-e2e-", "ci-hub-")):
        raise ValueError("cleanup only accepts ci-e2e-* or ci-hub-* test packages")
    hub = os.environ.get("COZY_HUB_URL", "")
    require_test_hub(hub, os.environ.get("COZY_TEST_HUB_ORIGIN", ""))
    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    route = "/v1/packages/" + "/".join(urllib.parse.quote(part, safe="") for part in (org, name))
    query = urllib.parse.urlencode({"hub": hub})
    request = urllib.request.Request("http://" + holder["address"] + route + "?" + query,
        method="DELETE", headers={"Authorization": "Bearer " + holder["token"]})
    try:
        with urllib.request.urlopen(request) as response:
            print(response.read().decode())
    except urllib.error.HTTPError as error:
        print(error.read().decode())
        return 0 if error.code == 404 else 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
