"""sound-joins.py OUT NAME...: each run's `h3 probe seam` and `h3 probe reach` log lines (sound-joins.sh), as JSON by run."""

import json
import sys
from pathlib import Path


def fields(text: str) -> dict[str, str]:
    return dict(token.split("=", 1) for token in text.split()[3:] if "=" in token)


def main() -> None:
    out, names = Path(sys.argv[1]), sys.argv[2:]
    found: dict[str, dict[str, object]] = {}
    for name in names:
        seams, reach = [], []
        path = out / f"{name}.ndjson"
        for line in path.read_text().splitlines() if path.exists() else []:
            try:
                event = json.loads(line)
            except ValueError:
                continue
            text = str(event.get("text") or "") if event.get("type") == "run.log" else ""
            if text.startswith("h3 probe seam "):
                seams.append(fields(text))
            elif text.startswith("h3 probe reach "):
                reach.append(fields(text))
        record = out / f"{name}.json"
        status = json.loads(record.read_text()).get("status") if record.exists() else None
        found[name] = {"status": status, "seams": seams, "reach": reach}
    print(json.dumps(found, indent=1))


if __name__ == "__main__":
    main()
