"""Each lane's quality against its bf16 reference, from the `quality` drive's files in a session's output folder.

    agreement.py <out dir>

`quality.refs` lines: `<model>@ <lane label> <reference label>`; `quality-<label>.json`: per text, its tokens, the top-1 next
token at each position and the perplexity. Prints a table: top-1 agreement over every position of every text, and both
perplexities (each text's, then their mean). Told, never judged: a lane's quality is read beside its speed."""

from __future__ import annotations

import json
import os
import sys


def main() -> int:
    out = sys.argv[1]

    def load(label: str) -> list[dict[str, object]] | None:
        try:
            with open(os.path.join(out, f"quality-{label}.json")) as f:
                return json.loads(f.read())
        except (OSError, ValueError):
            return None

    print("\n| lane | reference | top-1 agreement | perplexity (lane) | perplexity (bf16) |\n|---|---|---|---|---|")
    try:
        lines = open(os.path.join(out, "quality.refs")).read().split("\n")
    except OSError:
        lines = []
    for line in filter(None, lines):
        _, lane, ref = line.split()
        a, b = load(lane), load(ref)
        if a is None or b is None:
            print(f"| {lane} | {ref} | unread | | |")
            continue
        same = total = 0
        for x, y in zip(a, b):
            same += sum(p == q for p, q in zip(x["top1"], y["top1"]))  # type: ignore[call-overload]
            total += min(len(x["top1"]), len(y["top1"]))  # type: ignore[arg-type]
        ppl = lambda rows: " ".join(f"{r['ppl']:.3f}" for r in rows) + f" (mean {sum(r['ppl'] for r in rows) / len(rows):.3f})"  # type: ignore[misc]  # noqa: E731
        print(f"| {lane} | {ref} | {same / max(total, 1):.4f} ({same}/{total}) | {ppl(a)} | {ppl(b)} |")
    return 0


if __name__ == "__main__":
    sys.exit(main())
