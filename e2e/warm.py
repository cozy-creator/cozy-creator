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

# A warm names no project: its runs are Calcifer's own, read outside any project.
OUTSIDE = Path.home()


def main() -> int:
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
    request: dict[str, object] = {"target": args.target, "warm": args.level, "on": [args.on], "terms": args.terms, "loras": args.lora}
    target = Path(args.target).expanduser()
    if args.target.startswith((".", "/", "~")) or target.exists() or target.parent.exists():
        code, function = local_code(base, headers, target, args.stage_code)
        request.update(target=function, code=code)
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
        try:
            with urllib.request.urlopen(call) as response:
                answer = json.load(response)
        except urllib.error.HTTPError as error:
            said = error.read().decode()
            # `on` is a list since cl-873; a Calcifer before it takes one machine. Its version can't say which: an RC's dev
            # build says the release it follows (0.24.0+dev.g…), so its answer to the list does.
            if error.code != 400 or not isinstance(request["on"], list) or "sequence, expected a string" not in said:
                print(said, file=sys.stderr)
                return 1
            print(f"this Calcifer ({holder.get('version', '?')}) takes one machine: on {args.on}", file=sys.stderr)
            request["on"], key = args.on, str(uuid.uuid4())
            continue
        run = answer["run"]
        if not answer.get("preparing"):
            break
        subprocess.run([args.cozy, "--json", "run", "watch", run["id"]], stdout=sys.stderr, check=True, cwd=OUTSIDE)
    print(json.dumps({"type": "run.accepted", "run": run}), flush=True)
    return subprocess.call([args.cozy, "--json", "run", "watch", run["id"]], cwd=OUTSIDE)


def local_code(base: str, headers: dict[str, str], target: Path, stage_code: Path | None) -> tuple[dict[str, object], str]:
    """The code of a local package outside any project and the function it names, as the CLI sends it: captured by
    Calcifer from its folder. A Calcifer before native capture takes it by content (the stage-code example)."""
    package, function = (target, "") if target.is_dir() or target.suffix == ".py" else (target.parent, target.name)
    call = urllib.request.Request(base + "/v1/maintenance/source", data=json.dumps({"path": str(package.resolve())}).encode(),
        headers={**headers, "Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(call) as response:
            return json.load(response), function
    except urllib.error.HTTPError as error:
        if error.code not in (404, 405) or stage_code is None:
            raise
    staged = json.loads(subprocess.check_output([str(stage_code), str(target)], text=True))
    for left in staged["left"]:
        print("left out of code: " + left, file=sys.stderr)
    for file in staged["files"]:
        upload(base, headers, file["digest"], Path(file["path"]).read_bytes())
    upload(base, headers, staged["code"]["tree"], staged["document"].encode())
    return staged["code"], staged["target"]


def upload(base: str, headers: dict[str, str], digest: str, data: bytes) -> None:
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
