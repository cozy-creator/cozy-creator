"""sound-joins.py OUT NAME...: each run's `h3 join ...` log lines (sound-joins.sh), as JSON by run and seam."""

import json
import sys
from pathlib import Path

KINDS = ("shift", "window", "steps", "take")


def fields(text: str) -> dict[str, str]:
    return dict(token.split("=", 1) for token in text.split()[3:] if "=" in token)


def main() -> None:
    out, names = Path(sys.argv[1]), sys.argv[2:]
    found: dict[str, dict[str, object]] = {}
    for name in names:
        seams: dict[str, dict[str, str]] = {}
        take: dict[str, str] = {}
        path = out / f"{name}.ndjson"
        for line in path.read_text().splitlines() if path.exists() else []:
            try:
                event = json.loads(line)
            except ValueError:
                continue
            text = str(event.get("text") or "") if event.get("type") == "run.log" else ""
            words = text.split()
            if len(words) < 3 or words[:2] != ["h3", "join"] or words[2] not in KINDS:
                continue
            got = fields(text)
            if words[2] == "take":
                take = got
            else:
                seams.setdefault(got.get("segment", "?"), {}).update({f"{words[2]}.{key}": value for key, value in got.items() if key != "segment"})
        record = out / f"{name}.json"
        status = json.loads(record.read_text()).get("status") if record.exists() else None
        found[name] = {"status": status, "seams": seams, "take": take}
    print(json.dumps(found, indent=1))


if __name__ == "__main__":
    main()
