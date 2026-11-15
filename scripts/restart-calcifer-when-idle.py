#!/usr/bin/env python3
"""Restarts this home's Calcifer (its systemd user unit) once it is idle, as `scripts/dev-local.sh` does after it installs a
build (cl-962): while Calcifer serves a request (an upload, a publish, a run's submission, a page's write) or runs one of its own
(`here`: an upload, a publish), it waits, asking `GET /v1/activity` every `--every` seconds, for at most `--wait` seconds; then it
restarts it anyway and says so. Each line goes to stdout, which is the journal under systemd.

    restart-calcifer-when-idle.py [--wait 600] [--every 2] [--service cozy-calcifer.service]
"""

from __future__ import annotations

import argparse
import json
import subprocess
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlsplit


def say(line: str) -> None:
    print(f"{datetime.now(timezone.utc).isoformat(timespec='seconds')} {line}", flush=True)


def busy() -> str | None:
    """What keeps Calcifer from restarting now, or None when it is idle. One that can't say is busy."""
    holder_file = Path.home() / ".cozy/calcifer/calcifer.lock.holder"
    try:
        holder = json.loads(holder_file.read_text())
        address = urlsplit(f"http://{holder['address']}")
    except (OSError, KeyError, ValueError):
        return None  # no Calcifer serves: nothing to wait for
    if address.hostname not in ("127.0.0.1", "::1"):
        raise SystemExit("Calcifer's record does not name a loopback address")
    request = urllib.request.Request(
        f"http://{holder['address']}/v1/activity", headers={"Authorization": f"Bearer {holder['token']}"}
    )
    try:
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=5) as response:
            activity = json.load(response)
    except urllib.error.HTTPError as e:
        return f"cozy {holder.get('version', '?')} can't say what it serves ({e.code})"
    except (OSError, ValueError):
        return None  # it does not answer: a restart is what it needs
    uses = []
    if activity.get("requests"):
        uses.append(f"{activity['requests']} request(s) in flight")
    if activity.get("here"):
        uses.append(f"{activity['here']} of its own run(s) going (an upload, a publish)")
    return ", ".join(uses) or None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--wait", type=float, default=600, help="seconds to wait for Calcifer to be idle")
    parser.add_argument("--every", type=float, default=2, help="seconds between asks")
    parser.add_argument("--service", default="cozy-calcifer.service")
    args = parser.parse_args()
    began = time.monotonic()
    why = busy()
    said = None
    while why and time.monotonic() - began < args.wait:
        if why != said:
            say(f"Calcifer restart waits: {why}")
            said = why
        time.sleep(args.every)
        why = busy()
    waited = round(time.monotonic() - began)
    subprocess.run(["systemctl", "--user", "restart", args.service], check=True)
    if why:
        say(f"Calcifer restarted while busy after {waited}s, the most it waits: {why}")
    else:
        say(f"Calcifer restarted idle after {waited}s")


if __name__ == "__main__":
    main()
