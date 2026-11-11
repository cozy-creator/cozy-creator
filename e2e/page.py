"""What the page asks of a project that no CLI verb does, for the gate's fixtures (the current folder is the project; 0.24 has
no `cozy project` verbs). Each prints JSON on stdout; a refusal goes to stderr and exits 1.
    page.py import <project> <file> [--name N] [--description D]    the file as a box, as a drop on the canvas
    page.py yaml <project> <file>                                    project.yaml (format 4) read into the project
    page.py render <project> --on M[,M] [--n N] [--approve] [--pickup '<take>' FRAME] [--wait A,B] <name>...
                                                                     a render, followed until each name's (or --wait's) new
                                                                     versions land: one JSON line each, exit 1 on a failure
    page.py asset <project> <name>                                   the box and its versions
<project> is its folder, which names it in a route."""

import argparse
import hashlib
import json
from pathlib import Path
import sys
import time
from typing import cast
import urllib.error
import urllib.parse
import urllib.request
import uuid

Json = dict[str, object]
HOLDER = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
BASE, AUTH = "http://" + HOLDER["address"], {"Authorization": "Bearer " + HOLDER["token"]}
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))
# A render whose names show no run and no new version for this long made nothing.
IDLE_S = 600


def call(method: str, route: str, body: bytes | None = None, kind: str = "application/json", timeout: float = 60) -> Json:
    headers = {**AUTH, "Content-Type": kind}
    if method == "POST":
        headers["Idempotency-Key"] = str(uuid.uuid4())
    with OPENER.open(urllib.request.Request(BASE + route, data=body, method=method, headers=headers), timeout=timeout) as response:
        said = response.read()
    return cast(Json, json.loads(said)) if said.strip() else {}


def project(folder: str) -> str:
    return "/v1/projects/" + urllib.parse.quote(str(Path(folder).resolve()), safe="")


def held(data: bytes) -> str:
    digest = "sha256:" + hashlib.sha256(data).hexdigest()
    try:
        OPENER.open(urllib.request.Request(BASE + "/v1/files/" + digest, headers=AUTH, method="HEAD"), timeout=30).close()
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
        call("PUT", "/v1/files/" + digest, data, "application/octet-stream", timeout=600)
    return digest


def asset(folder: str, name: str) -> Json:
    return call("GET", project(folder) + "/assets/" + urllib.parse.quote(name, safe=""))


def render(args: argparse.Namespace) -> int:
    names: list[str] = args.names
    waited = args.wait.split(",") if args.wait else [name for name in names if name != "all"]
    before = {name: len(cast(list[object], asset(args.project, name)["versions"])) for name in waited}
    ask: Json = {"names": names, "n": args.n, "on": args.on.split(","), "approve": args.approve}
    if args.pickup:
        ask["pickup"] = {"take": args.pickup[0], "out_frame": int(args.pickup[1])}
    print(json.dumps({"asked": call("POST", project(args.project) + "/runs", json.dumps(ask).encode())}), flush=True)
    left, quiet, failed = dict(before), time.monotonic(), False
    while left:
        time.sleep(3)
        for name in list(left):
            state = asset(args.project, name)
            row, versions = cast(Json, state["asset"]), cast(list[Json], state["versions"])
            if row.get("making"):
                quiet = time.monotonic()
            if len(versions) >= left[name] + args.n or (row.get("failure") and not row.get("making")):
                print(json.dumps(state), flush=True)
                failed |= len(versions) < left[name] + args.n
                del left[name]
        if left and time.monotonic() - quiet > IDLE_S:
            print(f"nothing made of {', '.join(left)} in {IDLE_S} s", file=sys.stderr)
            return 1
    return 1 if failed else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    verbs = parser.add_subparsers(dest="verb", required=True)
    one = verbs.add_parser("import")
    one.add_argument("project")
    one.add_argument("file")
    one.add_argument("--name")
    one.add_argument("--description")
    one = verbs.add_parser("yaml")
    one.add_argument("project")
    one.add_argument("file")
    one = verbs.add_parser("render")
    one.add_argument("project")
    one.add_argument("--on", required=True)
    one.add_argument("--n", type=int, default=1)
    one.add_argument("--approve", action="store_true")
    one.add_argument("--pickup", nargs=2, metavar=("TAKE", "FRAME"))
    one.add_argument("--wait")
    one.add_argument("names", nargs="+")
    one = verbs.add_parser("asset")
    one.add_argument("project")
    one.add_argument("name")
    args = parser.parse_args()
    if args.verb == "import":
        ask = {"source": held(Path(args.file).read_bytes())}
        ask |= {key: value for key, value in (("name", args.name), ("description", args.description)) if value}
        print(json.dumps(call("POST", project(args.project) + "/imports", json.dumps(ask).encode())))
    elif args.verb == "yaml":
        print(json.dumps(call("PUT", project(args.project) + "/yaml", Path(args.file).read_bytes(), "application/yaml")))
    elif args.verb == "render":
        return render(args)
    else:
        print(json.dumps(asset(args.project, args.name)))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except urllib.error.HTTPError as error:
        print(error.read().decode(), file=sys.stderr)
        sys.exit(1)
