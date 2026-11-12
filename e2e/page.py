"""What the page asks of a project that no CLI verb does, for the gate's fixtures (the current folder is the project; 0.24 has
no `cozy project` verbs). Each prints JSON on stdout; a refusal goes to stderr and exits 1.
    page.py import <project> <file> [--name N] [--description D]    the file as a box, as a drop on the canvas
    page.py add <project> <name> --tool T [--fields JSON]           a tool's box, as a drop on the canvas (`<name>` exactly)
    page.py yaml <project> [file]                                    project.yaml (format 4) read into the project; no file:
                                                                     the project as YAML
    page.py render <project> --on M[,M]|auto [--n N] [--approve] [--pickup '<take>' FRAME] [--wait A,B] [--queue] [--missing] <name>...
                                                                     a render, followed until each name's (or --wait's) new
                                                                     versions land: one JSON line each, exit 1 on a failure.
                                                                     `auto`: what the page sends for Automatically; --queue:
                                                                     asked, not followed; --missing: only what has no state
    page.py approve <project> <name> <version>                       a version approved, as a double tap on its take does
    page.py asset <project> <name>...                                each box and its versions, a JSON line each
    page.py board <project>                                          the project's render as the page shows it
    page.py cancel <project> <name>...                               their jobs canceled, as the page cancels them
    page.py tools <project>                                          the tools a box can be, as the page lists them
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
# What a version's main file is, and how long it may take to land here once the version stands.
MEDIA, LAND_S = ("image", "video", "audio"), 900


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


def read(folder: str) -> Json:
    return call("GET", project(folder) + "?assets=false")


def automatic(folder: str) -> list[str]:
    """What the page sends with no machine green in its machines panel (web/app CanvasHost.tsx `useMachines`): the project's machines, else every
    rental this computer ordered that's up or booting, another computer's that's up, and this one last where it runs models."""
    chosen = cast(list[str], read(folder).get("machines", []))
    if chosen:
        return chosen
    listed = cast(list[Json], call("GET", "/v1/machines")["machines"])
    held = [machine for machine in listed if machine.get("ordered", True) and not machine.get("offline")]
    rented = [machine for machine in held if machine["kind"] == "rental" and machine["state"] in ("ready", "booting")]
    up = [machine for machine in held if machine["kind"] != "rental" and machine["state"] == "ready" and not machine.get("unreachable")]
    others = [machine for machine in up if machine["kind"] != "local"]
    here = [machine for machine in up if machine["kind"] == "local"] if call("GET", "/v1/status").get("local_usable") else []
    return [str(machine["name"]) for machine in rented + others + here]


def render(args: argparse.Namespace) -> int:
    names: list[str] = args.names
    on = automatic(args.project) if args.on == "auto" else args.on.split(",")
    waited = [] if args.queue else args.wait.split(",") if args.wait else [name for name in names if name != "all"]
    before = {name: len(cast(list[object], asset(args.project, name)["versions"])) for name in waited}
    if args.missing:  # a box with a version already makes nothing new
        before = {name: count for name, count in before.items() if not count}
    ask: Json = {"names": names, "n": args.n, "on": on, "approve": args.approve, "missing": args.missing}
    if args.pickup:
        ask["pickup"] = {"take": args.pickup[0], "out_frame": int(args.pickup[1])}
    print(json.dumps({"on": on, "asked": call("POST", project(args.project) + "/runs", json.dumps(ask).encode())}), flush=True)
    left, quiet, failed, made = dict(before), time.monotonic(), False, dict[str, float]()
    while left:
        time.sleep(3)
        # A box made here (an edit) has no run: its job on the board, and how far it's rendered, say it's going.
        going = {cast(Json, job).get("asset") for job in cast(list[object], cast(Json, read(args.project).get("board") or {}).get("jobs", []))
                 if cast(Json, job).get("state") in ("waiting", "running")}
        for name in list(left):
            state = asset(args.project, name)
            row, versions = cast(Json, state["asset"]), cast(list[Json], state["versions"])
            if row.get("making") or row.get("rendering") is not None or name in going:
                quiet = time.monotonic()
            new, absent = versions[left[name]:], list[str]()
            if len(new) >= args.n:
                # A version stands before its picture, clip or sound has landed here: followed until it has (LAND_S at most).
                made.setdefault(name, time.monotonic())
                absent = [key for version in new for key, file in cast(Json, version["files"]).items() if key in MEDIA and cast(Json, file).get("absent")]
                if absent and time.monotonic() - made[name] < LAND_S:
                    quiet = time.monotonic()
                    continue
                if absent:
                    print(f"{name}: {', '.join(absent)} not here {LAND_S} s after its version stood", file=sys.stderr)
            if len(new) >= args.n or (row.get("failure") and not row.get("making")):
                print(json.dumps(state), flush=True)
                failed |= len(new) < args.n or bool(absent)
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
    one = verbs.add_parser("add")
    one.add_argument("project")
    one.add_argument("name")
    one.add_argument("--tool", required=True)
    one.add_argument("--fields", default="{}")
    one = verbs.add_parser("yaml")
    one.add_argument("project")
    one.add_argument("file", nargs="?")
    one = verbs.add_parser("render")
    one.add_argument("project")
    one.add_argument("--on", required=True)
    one.add_argument("--n", type=int, default=1)
    one.add_argument("--approve", action="store_true")
    one.add_argument("--pickup", nargs=2, metavar=("TAKE", "FRAME"))
    one.add_argument("--wait")
    one.add_argument("--queue", action="store_true")
    one.add_argument("--missing", action="store_true")
    one.add_argument("names", nargs="+")
    one = verbs.add_parser("approve")
    one.add_argument("project")
    one.add_argument("name")
    one.add_argument("version")
    one = verbs.add_parser("asset")
    one.add_argument("project")
    one.add_argument("names", nargs="+")
    for verb in ("board", "tools"):
        verbs.add_parser(verb).add_argument("project")
    one = verbs.add_parser("cancel")
    one.add_argument("project")
    one.add_argument("names", nargs="+")
    args = parser.parse_args()
    if args.verb == "import":
        ask = {"source": held(Path(args.file).read_bytes())}
        ask |= {key: value for key, value in (("name", args.name), ("description", args.description)) if value}
        print(json.dumps(call("POST", project(args.project) + "/imports", json.dumps(ask).encode())))
    elif args.verb == "add":
        drop: Json = {"tool": args.tool, "name": args.name, "exact": True, "fields": json.loads(args.fields)}
        print(json.dumps(call("POST", project(args.project) + "/assets", json.dumps(drop).encode())))
    elif args.verb == "yaml" and args.file:
        print(json.dumps(call("PUT", project(args.project) + "/yaml", Path(args.file).read_bytes(), "application/yaml")))
    elif args.verb == "yaml":
        with OPENER.open(urllib.request.Request(BASE + project(args.project) + "/yaml", headers=AUTH), timeout=60) as response:
            sys.stdout.write(response.read().decode())
    elif args.verb == "render":
        return render(args)
    elif args.verb == "approve":
        print(json.dumps(call("POST", project(args.project) + "/approve", json.dumps({"name": args.name, "version": args.version}).encode())))
    elif args.verb == "cancel":
        print(json.dumps(call("POST", project(args.project) + "/cancel", json.dumps({"names": args.names}).encode())))
    elif args.verb == "board":
        print(json.dumps(read(args.project).get("board")))
    elif args.verb == "tools":
        print(json.dumps(call("GET", "/v1/tools?" + urllib.parse.urlencode({"project": str(read(args.project)["id"])}))))
    else:
        for name in args.names:
            print(json.dumps(asset(args.project, name)))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except urllib.error.HTTPError as error:
        print(error.read().decode(), file=sys.stderr)
        sys.exit(1)
