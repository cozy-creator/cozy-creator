"""Read model metadata through Calcifer for fixture assertions."""
import json
import os
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request

name, _, release = sys.argv[1].partition("@")
org, model = name.split("/", 1) if "/" in name else ("_", name)
holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
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
    sys.exit(1)
