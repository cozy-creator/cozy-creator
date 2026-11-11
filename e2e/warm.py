"""Submit an API warming request for a release-gate fixture and follow its run."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import urllib.error
import urllib.request
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target")
    parser.add_argument("level", choices=("installed", "downloaded", "imported", "host", "gpu", "off"))
    parser.add_argument("--on", required=True)
    parser.add_argument("--cozy", default="cozy")
    parser.add_argument("--lane")
    parser.add_argument("--lora", action="append", default=[])
    parser.add_argument("terms", nargs="*")
    args = parser.parse_intermixed_args()
    if args.target.startswith((".", "/", "~")) or Path(args.target).exists() or Path(args.target).parent.exists():
        parser.error("API warming needs a published target; use a published or pinned candidate for local-package gate preparation")
    if any(not term.startswith("model.") or "=" not in term for term in args.terms):
        parser.error("warming accepts only model.<slot>=<ref> terms")

    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    request = {"target": args.target, "warm": args.level, "on": args.on, "terms": args.terms, "loras": args.lora}
    if args.lane:
        request["lane"] = args.lane
    if hub := os.environ.get("COZY_HUB_URL"):
        request["hub"] = hub
    key = str(uuid.uuid4())
    while True:
        call = urllib.request.Request(
            "http://" + holder["address"] + "/v1/runs",
            data=json.dumps(request).encode(),
            headers={"Authorization": "Bearer " + holder["token"], "Content-Type": "application/json", "Idempotency-Key": key},
        )
        with urllib.request.urlopen(call) as response:
            answer = json.load(response)
        run = answer["run"]
        if not answer.get("preparing"):
            break
        subprocess.run([args.cozy, "--json", "run", "watch", str(run["number"])], stdout=sys.stderr, check=True)
    print(json.dumps({"type": "run.accepted", "run": run}), flush=True)
    return subprocess.call([args.cozy, "--json", "run", "watch", str(run["number"])])


if __name__ == "__main__":
    try:
        sys.exit(main())
    except urllib.error.HTTPError as error:
        print(error.read().decode(), file=sys.stderr)
        sys.exit(1)
