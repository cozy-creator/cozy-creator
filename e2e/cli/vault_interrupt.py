"""Interrupt a real encryption midway: Calcifer is killed while it copies into its stage.

Runs only inside the cloud project-vault fixture. Prints what was left at the moment of the kill;
the shard then proves the next start recovers all of it.
"""
import json
import os
from pathlib import Path
import signal
import sys
import time
from typing import cast
import urllib.parse
import urllib.request
import uuid

sys.path.insert(0, str(Path(__file__).parent))
from vault_stream import events  # noqa: E402


def main() -> None:
    project, credential = Path(sys.argv[1]).resolve(), Path(sys.argv[2])
    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    url = f"http://{holder['address']}/v1/projects/{urllib.parse.quote(str(project), safe='')}/vault/operations/{uuid.uuid4().hex}"
    body = json.dumps({"action": "encrypt", "passphrase": credential.read_text().strip(), "idle_timeout_s": 0}).encode()
    headers = {"Authorization": f"Bearer {holder['token']}", "Content-Type": "application/json", "Accept": "text/event-stream"}
    request = urllib.request.Request(url, data=body, headers=headers, method="PUT")
    seen: dict[str, int] = {}
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=300) as stream:
        for event in events(stream):
            fields = cast(dict[str, object], event)
            if fields.get("type") in ("completed", "failed"):
                raise AssertionError(f"the conversion ended ({fields.get('type')}) before it could be interrupted")
            done, total = fields.get("bytes_done"), fields.get("bytes_total")
            if fields.get("type") == "progress" and fields.get("phase") == "copying" \
                    and isinstance(done, int) and isinstance(total, int) and 0 < done < total:
                seen = {"bytes_done": done, "bytes_total": total}
                os.kill(int(holder["pid"]), signal.SIGKILL)
                break
    assert seen, "the conversion never reported copying"
    while True:
        try:
            os.kill(int(holder["pid"]), 0)
        except ProcessLookupError:
            break
        time.sleep(0.1)
    parent = project.parent
    stages = sorted(path.name for path in parent.glob(".cozy-vault-stage-*"))
    receipts = sorted(path.name for path in parent.glob(".cozy-vault-*.transaction.json"))
    runtime = sorted(path.name for path in (Path.home() / ".cozy/calcifer/vaults").glob("stage-*"))
    assert len(stages) == 1 and len(receipts) == 1, f"no half-made stage to recover: {stages} {receipts}"
    print(json.dumps({"interrupted": seen, "stages": len(stages), "receipts": len(receipts), "runtime": len(runtime)}))


if __name__ == "__main__":
    main()
