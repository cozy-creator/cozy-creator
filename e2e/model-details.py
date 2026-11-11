"""Read model metadata through Calcifer for fixture assertions."""
import json
import os
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request


def main() -> int:
    name, _, release = sys.argv[1].partition("@")
    org, model = name.split("/", 1) if "/" in name else ("_", name)
    holder: dict[str, str] = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    query = {"hub": os.environ.get("COZY_HUB_URL", "")}
    if release:
        query["release"] = release
    url = "http://" + holder["address"] + "/v1/models/" + "/".join(urllib.parse.quote(part, safe="") for part in (org, model))
    request = urllib.request.Request(url + "?" + urllib.parse.urlencode(query), headers={"Authorization": "Bearer " + holder["token"]})
    try:
        with urllib.request.urlopen(request) as response:
            print(response.read().decode())
    except urllib.error.HTTPError as error:
        print(error.read().decode())
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
