"""The installed generation's interpreter, preserving its env/source links."""

import json
from pathlib import Path
import re
import sqlite3
import sys


def normalized(name: str) -> str:
    return re.sub(r"[-_.]+", "_", name).lower()


home, receipt, package, version = sys.argv[1:]
events = [json.loads(line) for line in Path(receipt).read_text().splitlines() if line.startswith("{")]
aliases = [event.get("run", {}).get("result", {}).get("installation") for event in events if event.get("type") == "run.completed"]
aliases = [alias for alias in aliases if alias]
if len(aliases) != 1:
    sys.exit(f"{package}: the install receipt names {len(aliases)} installations")

found = []
for journal in Path(home).rglob("executions.sqlite3"):
    with sqlite3.connect(journal.as_uri() + "?mode=ro", uri=True) as db:
        for (raw,) in db.execute("SELECT record FROM installations WHERE alias=?", (aliases[0],)):
            installed = json.loads(raw)
            if not installed.get("actor") or normalized(installed["package"].rsplit("/", 1)[-1]) != normalized(package) or installed["release"] != version:
                sys.exit(f"{package}: the install receipt's owner, package or version does not match")
            path = journal.parent.parent / "generations" / installed["generation"] / "generation.json"
            generation = json.loads(path.read_text())
            if generation["identity"] != installed["generation"] or normalized(generation["package"]) != normalized(package) or generation["version"] != version:
                sys.exit(f"{package}: the installed generation's identity, package or version does not match")
            found.append(generation["python"])

if len(found) != 1:
    sys.exit(f"{package}: the install receipt resolves to {len(found)} owned generations")
print(found[0])
