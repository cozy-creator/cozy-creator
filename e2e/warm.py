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
    parser.add_argument("--stage-code", type=Path)
    parser.add_argument("--lane")
    parser.add_argument("--lora", action="append", default=[])
    parser.add_argument("terms", nargs="*")
    args = parser.parse_intermixed_args()
    if any(not term.startswith("model.") or "=" not in term for term in args.terms):
        parser.error("warming accepts only model.<slot>=<ref> terms")

    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    base = "http://" + holder["address"]
    headers = {"Authorization": "Bearer " + holder["token"]}
    request = {"target": args.target, "warm": args.level, "on": args.on, "terms": args.terms, "loras": args.lora}
    target = Path(args.target).expanduser()
    if args.target.startswith((".", "/", "~")) or target.exists() or target.parent.exists():
        if args.stage_code is None:
            parser.error("local warming needs --stage-code with the api stage-code example executable")
        staged = json.loads(subprocess.check_output([str(args.stage_code), str(target)], text=True))
        for left in staged["left"]:
            print("left out of code: " + left, file=sys.stderr)
        for file in staged["files"]:
            upload(base, headers, file["digest"], Path(file["path"]).read_bytes())
        upload(base, headers, staged["code"]["tree"], staged["document"].encode())
        request.update(target=staged["target"], code=staged["code"])
    if args.lane:
        request["lane"] = args.lane
    if hub := os.environ.get("COZY_HUB_URL"):
        request["hub"] = hub
    key = str(uuid.uuid4())
    while True:
        call = urllib.request.Request(
            base + "/v1/runs",
            data=json.dumps(request).encode(),
            headers={**headers, "Content-Type": "application/json", "Idempotency-Key": key},
        )
        with urllib.request.urlopen(call) as response:
            answer = json.load(response)
        run = answer["run"]
        if not answer.get("preparing"):
            break
        subprocess.run([args.cozy, "--json", "run", "watch", str(run["number"])], stdout=sys.stderr, check=True)
    print(json.dumps({"type": "run.accepted", "run": run}), flush=True)
    return subprocess.call([args.cozy, "--json", "run", "watch", str(run["number"])])


def upload(base, headers, digest, data):
    url = base + "/v1/files/" + digest
    try:
        with urllib.request.urlopen(urllib.request.Request(url, headers=headers, method="HEAD")):
            return
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
    call = urllib.request.Request(url, data=data, method="PUT", headers={
        **headers, "Content-Type": "application/octet-stream", "Content-Length": str(len(data)),
    })
    with urllib.request.urlopen(call):
        pass


if __name__ == "__main__":
    try:
        sys.exit(main())
    except urllib.error.HTTPError as error:
        print(error.read().decode(), file=sys.stderr)
        sys.exit(1)
