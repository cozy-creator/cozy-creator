"""Calcifer's quotes, as the page asks them (`POST /v1/run-estimates`): when each machine a run may go to would finish it."""

import argparse
import hashlib
import json
from pathlib import Path
import sys
import urllib.error
import urllib.request
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from warm import upload  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target")
    parser.add_argument("--on", required=True, help="machines, comma-separated")
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--file", action="append", default=[], help="NAME=FILE")
    parser.add_argument("--count", type=int, default=1)
    args = parser.parse_args()
    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    base = "http://" + holder["address"]
    headers = {"Authorization": "Bearer " + holder["token"]}
    files, paths = [], {}
    for attached in args.file:
        name, _, written = attached.partition("=")
        data = Path(written).read_bytes()
        digest = "sha256:" + hashlib.sha256(data).hexdigest()
        upload(base, headers, digest, data)
        files.append({"name": name, "file": written})
        paths[written] = {"file": digest}
    request = {"target": args.target, "on": args.on.split(","), "files": files, "paths": paths,
               "project": str(args.project.resolve(strict=True))}
    call = urllib.request.Request(f"{base}/v1/run-estimates?count={args.count}", data=json.dumps(request).encode(), headers={
        **headers, "Content-Type": "application/json", "Idempotency-Key": str(uuid.uuid4()),
    })
    with urllib.request.urlopen(call) as response:
        print(json.dumps(json.load(response)))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except urllib.error.HTTPError as error:
        print(error.read().decode(), file=sys.stderr)
        sys.exit(1)
