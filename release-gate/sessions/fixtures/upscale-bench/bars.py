"""Holds candidates to their bars over metrics.py's scores.

    bars.py <scores.tsv> '<bars JSON>'

A bar is {candidate, vs, metric, by}: `candidate`'s mean `metric` (dists or lpips, lower better) over the full-reference
pictures it shares with `vs` (a candidate, or `lanczos`, the floor) is at most `vs`'s mean times 1 - `by`. One line a bar,
PASS or FAIL with both means; exits 1 when any fails or has no pictures to judge.
"""

import csv
import json
import sys

rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
scores: dict[str, dict[str, dict[str, float]]] = {}
for r in rows:
    if r["kind"] != "paul" and r.get("dists"):
        scores.setdefault(r["candidate"], {})[r["image"]] = {"dists": float(r["dists"]), "lpips": float(r["lpips"])}
failed = False
for bar in json.loads(sys.argv[2]):
    name, vs, metric, by = bar["candidate"], bar["vs"], bar["metric"], float(bar["by"])
    shared = sorted(set(scores.get(name, {})) & set(scores.get(vs, {})))
    if not shared:
        print(f"FAIL {name} vs {vs}: no full-reference pictures scored for both")
        failed = True
        continue
    mine = sum(scores[name][i][metric] for i in shared) / len(shared)
    theirs = sum(scores[vs][i][metric] for i in shared) / len(shared)
    ok = mine <= theirs * (1 - by)
    failed |= not ok
    print(f"{'PASS' if ok else 'FAIL'} {name} vs {vs}: {metric} {mine:.4f} against {theirs:.4f} over {len(shared)} pictures "
          f"({(1 - mine / theirs) * 100:+.1f}% better; the bar {by * 100:.0f}%)")
sys.exit(1 if failed else 0)
