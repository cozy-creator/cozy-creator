"""Bind a local CPU fixture through Calcifer without executing its function."""

import argparse
import hashlib
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
    parser.add_argument("target", type=Path)
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--file", action="append", default=[])
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--cozy", required=True)
    parser.add_argument("--stage-code", default=os.environ.get("RUN_CODE_STAGE"), required=not os.environ.get("RUN_CODE_STAGE"))
    args = parser.parse_args()
    hub = os.environ.get("COZY_HUB_URL", "")
    require_ci_test_hub(hub)
    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    base = "http://" + holder["address"]
    headers = {"Authorization": "Bearer " + holder["token"], "cozy-project-storage": "3"}
    staged = json.loads(subprocess.check_output([args.stage_code, str(args.target)], text=True))
    for left in staged["left"]:
        print("left out of code: " + left, file=sys.stderr)
    for file in staged["files"]:
        upload(base, headers, file["digest"], Path(file["path"]).read_bytes())
    upload(base, headers, staged["code"]["tree"], staged["document"].encode())

    payload = json.loads(args.input.read_text())
    paths: dict[str, dict[str, str]] = {}

    def attach(written: str, directory: Path, required: bool = False) -> None:
        path = Path(written).expanduser()
        if not path.is_absolute():
            path = directory / path
        try:
            present = path.is_file()
        except OSError:  # A prompt's text is not a filename.
            present = False
        if not present:
            if required:
                raise ValueError(f"input file not found: {written}")
            return
        with path.open("rb") as source:
            digest = "sha256:" + hashlib.file_digest(source, "sha256").hexdigest()
        upload(base, headers, digest, path.read_bytes())
        paths[written] = {"file": digest}

    def input_paths(value: object) -> None:
        if isinstance(value, str):
            attach(value, args.input.resolve().parent)
        elif isinstance(value, list):
            for item in value:
                input_paths(item)
        elif isinstance(value, dict):
            for item in value.values():
                input_paths(item)

    input_paths(payload)
    files = []
    for attached in args.file:
        name, separator, file = attached.partition("=")
        if not separator:
            parser.error("--file needs NAME=FILE")
        attach(file, args.project.resolve(), required=True)
        files.append({"name": name, "file": file})
    request = {"target": staged["target"], "code": staged["code"], "input": payload,
               "files": files, "paths": paths, "on": ["local"], "hub": hub,
               "project": str(args.project.resolve(strict=True))}
    key = str(uuid.uuid4())
    followed: set[int] = set()
    while True:
        call = urllib.request.Request(base + "/v1/run-estimates", data=json.dumps(request).encode(), headers={
            **headers, "Content-Type": "application/json", "Idempotency-Key": key,
        })
        with urllib.request.urlopen(call) as response:
            answer = json.load(response)
        if "error" in answer:
            print(json.dumps(answer))
            return 1
        preparing = answer.get("preparing")
        if not preparing:
            if not isinstance(answer.get("machines"), list):
                raise ValueError("Calcifer returned no estimate")
            print(json.dumps(answer))
            return 0
        number = preparing["number"]
        # Local preparation installs code with a WARM run; the requested model function never runs.
        if preparing["kind"] != "warm" or number in followed:
            raise ValueError("Calcifer did not finish the local-code preparation")
        followed.add(number)
        subprocess.run([args.cozy, "--json", "run", "watch", str(number)], stdout=sys.stderr, check=True)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except urllib.error.HTTPError as error:
        print(error.read().decode())
        sys.exit(1)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(json.dumps({"error": {"message": str(error)}}))
        sys.exit(1)
