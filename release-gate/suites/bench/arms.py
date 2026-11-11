#!/usr/bin/env python3
"""Which end-to-end arms a new attention table needs (xs-171, measurement-design §3.3).

    arms.py OLD NEW     two attention_table.json files: the release's and the candidate's

Every 8- or 4-bit cell whose head moved in any band, then a release-gate.yml dispatch per class it moved on, running the arms:
each model that declares that level, on the class's reference card (arms-skus), on the table's kernels (no pin) beside the
same request and seed at `--attention-kernel exact`. bench.py holds each exact arm's output to its pair's (`like`). 16-bit
cells need no arm: their kernels differ by BF16 rounding."""

import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
#: The arm cells (cells.tsv) of the models that declare each level (the packages' `attention_bits`): H3's transformer and
#: VAE, and Qwen-Image 2.1's, MOSS's and Klein's image blocks, at 8. Nothing declares 4 (Paul 11-11: 4-bit attention only
#: by pin); a model that opts in adds its cells here.
ARMS: dict[int, list[str]] = {
    4: [],
    8: ["a-h3-15s", "a-h3-15s-b", "a-qwen4mp", "a-qwen4mp-b", "a-moss", "a-moss-b", "a-klein", "a-klein-b"],
}


def band(edges: list[int], b: int) -> str:
    """runtime-design §8's band label."""
    if b == 0:
        return f"<{edges[0]}" if edges else ""
    return f">={edges[-1]}" if b == len(edges) else f"{edges[b - 1]}-{edges[b] - 1}"


def heads(path: str) -> dict[tuple[str, str, int, int | None, str], str]:
    """Each (class, layout, level, head dim, band) -> its head. Tables with other edges compare band by band label."""
    table = json.load(open(path))
    edges: list[int] = table["tokens"]
    found = {}
    for cell in table["cells"]:
        for b, ranking in enumerate(cell["order"]):
            found[(cell["class"], cell["layout"], cell["level"], cell["head_dim"], band(edges, b))] = ranking[0]
    return found


def groups() -> dict[str, list[str]]:
    """class -> its arms-skus row: group, host GiB, disk GB, hard stop min."""
    found = {}
    for line in open(os.path.join(HERE, "arms-skus")):
        words = line.split()
        if words and not line.startswith("#"):
            found[words[4]] = words[:4]
    return found


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    old, new = heads(argv[1]), heads(argv[2])
    moved: dict[str, set[int]] = {}
    for key in sorted(set(old) | set(new), key=str):
        klass, layout, level, dim, label = key
        if level < 16 and old.get(key) != new.get(key):
            print(f"{klass} {layout} {level} d{dim or '*'} {label or 'all'}: {old.get(key, '-')} -> {new.get(key, '-')}")
            moved.setdefault(klass, set()).add(level)
    if not moved:
        print("no 8- or 4-bit head moved: no arms")
        return 0
    where = groups()
    for klass, levels in sorted(moved.items()):
        cells = sorted({f"{c}{arm}" for level in levels for c in ARMS.get(level, []) for arm in ("", "-exact")})
        if not cells:
            print(f"{klass}: no model declares level {sorted(levels)}: no arms")
            continue
        group, _, disk, cap = where[klass]
        print(f"gh workflow run release-gate.yml -R cozy-creator/cozy-creator --ref <open rc> -f hub=prod -f suites=bench "
              f"-f skus={group.split('.')[0].rsplit('x', 1)[0]} -f group={group} -f disk={disk} -f cap_minutes={cap} -f only={','.join(cells)} "
              "-f establish=true -f publish=false -f runtime_wheel=<candidate>")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
