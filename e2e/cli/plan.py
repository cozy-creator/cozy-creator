"""Calcifer's quotes, as the page asks them (`POST /v1/run-estimates`): when each machine a run may go to would finish it."""

import argparse
import hashlib
import json
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid

def upload(base: str, headers: dict[str, str], digest: str, data: bytes, project: Path) -> None:
    """The file into its project's own content store, as the page sends one (`PUT /v1/project-files/<digest>`), unless it holds it."""
    url = base + "/v1/project-files/" + digest + "?project=" + urllib.parse.quote(str(project), safe="")
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
    headers = {"Authorization": "Bearer " + holder["token"], "cozy-project-storage": "3"}
    files: list[dict[str, str]] = []
    paths: dict[str, dict[str, str]] = {}
    for attached in args.file:
        name, _, written = attached.partition("=")
        data = Path(written).read_bytes()
        digest = "sha256:" + hashlib.sha256(data).hexdigest()
        upload(base, headers, digest, data, args.project.resolve(strict=True))
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
