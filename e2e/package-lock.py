"""Exercise Calcifer's retained package-lock API without a public CLI command."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import urllib.error
import urllib.request
import uuid

from test_hub import require_ci_test_hub
from warm import upload


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--stage-code", default=os.environ.get("RUN_CODE_STAGE"), required=not os.environ.get("RUN_CODE_STAGE"))
    parser.add_argument("--cozy", default=os.environ.get("COZY", "cozy"))
    parser.add_argument("--root", action="store_true")
    parser.add_argument("--upgrade-package", action="append", default=[])
    args = parser.parse_args()
    hub = os.environ.get("COZY_HUB_URL", "")
    require_ci_test_hub(hub)
    directory = args.directory.resolve(strict=True)
    staged = json.loads(subprocess.check_output([args.stage_code, str(directory)], text=True))
    for left in staged["left"]:
        print("left out of code: " + left, file=sys.stderr)
    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    base = "http://" + holder["address"]
    headers = {"Authorization": "Bearer " + holder["token"]}
    for file in staged["files"]:
        upload(base, headers, file["digest"], Path(file["path"]).read_bytes())
    upload(base, headers, staged["code"]["tree"], staged["document"].encode())
    request = {"target": "cozy/package_lock", "code": staged["code"], "hub": hub,
        "input": {"root": args.root, "upgrade": args.upgrade_package}}
    call = urllib.request.Request(base + "/v1/runs", data=json.dumps(request).encode(), headers={
        **headers, "Content-Type": "application/json", "Idempotency-Key": str(uuid.uuid4()),
    })
    with urllib.request.urlopen(call) as response:
        run = json.load(response)["run"]
    if run["status"] not in {"completed", "failed", "canceled"}:
        followed = subprocess.run([args.cozy, "--json", "--full", "run", "watch", str(run["number"])],
            capture_output=True, text=True)
        print(followed.stderr, file=sys.stderr, end="")
        for line in followed.stdout.splitlines():
            value = json.loads(line)
            candidate = value.get("run", value)
            if candidate.get("number") == run["number"]:
                run = candidate
    result = run.get("result") or {}
    if run["status"] != "completed":
        print(json.dumps(result if "error" in result else {"error": run.get("reason")}), file=sys.stderr)
        return 1
    path = directory / "uv.lock"
    if result["changed"]:
        temporary = path.with_name("uv.lock." + uuid.uuid4().hex)
        try:
            temporary.write_bytes(result["lock"].encode())
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
    print(json.dumps({**result, "lock": str(path)}))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except urllib.error.HTTPError as error:
        print(error.read().decode(), file=sys.stderr)
        sys.exit(1)
