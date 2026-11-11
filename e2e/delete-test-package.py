"""Clean up a release-suite throwaway through the retained administration API."""
import json
import os
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request


def main() -> int:
    org, name = sys.argv[1].split("/", 1)
    if not name.startswith("ci-e2e-"):
        raise ValueError("cleanup only accepts the release suite's ci-e2e-* packages")
    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    route = "/v1/packages/" + "/".join(urllib.parse.quote(part, safe="") for part in (org, name))
    query = urllib.parse.urlencode({"hub": os.environ.get("COZY_HUB_URL", "")})
    request = urllib.request.Request("http://" + holder["address"] + route + "?" + query,
        method="DELETE", headers={"Authorization": "Bearer " + holder["token"]})
    try:
        with urllib.request.urlopen(request) as response:
            print(response.read().decode())
    except urllib.error.HTTPError as error:
        print(error.read().decode())
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
