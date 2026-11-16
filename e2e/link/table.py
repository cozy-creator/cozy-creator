"""tfs-300: link-metric records (e2e/link/judge.py) side by side, with each row's verdict.

  table.py BASE_DIR [CAND_DIR]      every *.json record under each (one per scenario per run); markdown on stdout

A row's value on a side is its median over that side's runs; `spread` is max - min. The bars, fixed 11-16 before any candidate
was measured (coordinator's word): a row is a tie when the difference is within 5% of the baseline OR within the row's
instrument band below; a judged row worse by more fails. A row whose own baseline spread is wider than its tie band can't judge
(`noisy`). Every guarantee must hold on every candidate run. Unjudged rows are reported only, unless the design claims them."""

import json, statistics, sys
from fnmatch import fnmatch
from pathlib import Path

# (pattern, direction, band in the row's unit, judged): direction -1 lower is better, +1 higher; first match wins.
# The outcomes, shown first (coordinator 11-16): accept to first output, install time, the weights' time against the link's
# ceiling, control-call latency at p99.
OUTCOMES = ["*.ttfo_s", "cold.*.install_s", "*vs_ceiling", "*.probe_p99_ms", "*.read_s", "*.fetch_s", "*.release_s", "*.both_s"]

BARS = [
    ("two.*_s", 0, 0, False), ("two.*_ratio", 0, 0, False), ("two.both_s", -1, 1.0, True),
    ("*.share_a", 0, 0, False), ("*.jain", 0, 0, False), ("*.progress_gap", 0, 0, False),
    ("*.install_quiet_s", 0, 0, False), ("*.queue_p95_ms", 0, 0, False), ("*.cancel_call_s", 0, 0, False),
    ("*.fetch_share", 0, 0, False), ("*.weights_install_share", 0, 0, False), ("*.ahead_share_during_fetch", 0, 0, False),
    ("*.link_mb_s", 0, 0, False),
    ("*.disk_mb_s", 1, 0.1, True),
    ("*longest_zero_s", -1, 2.0, True),
    ("*.idle_wanted_s", -1, 2.0, True),
    ("*_s", -1, 1.0, True),
    ("*_ratio", -1, 0.02, True),
    ("*.goodput", -1, 0.01, True),
    ("*.link_to_disk", -1, 0.02, True),
    ("*.link_use", 1, 0.02, True),
    ("*vs_ceiling", 1, 0.02, True),
    ("*_share", 1, 0.02, True),
    ("*.disk_to_cap", 1, 0.02, True),
    ("*_mib", -1, 64, True),
    ("*.probe_failures", -1, 0, True),
    ("*_ms", -1, 2, True),
]


def bar(row):
    return next((b for b in BARS if fnmatch(row, b[0])), (row, 0, 0, False))


def load(d):
    rows, guarantees, notes = {}, {}, []
    for p in sorted(Path(d).rglob("*.json")):
        try:
            rec = json.loads(p.read_text())
        except ValueError:
            continue
        if rec.get("schema") != "link-metrics/1":
            continue
        for k, v in rec["rows"].items():
            if isinstance(v, (int, float)):
                rows.setdefault(k, []).append(float(v))
        for k, v in rec["guarantees"].items():
            guarantees.setdefault(k, []).append(v)
        notes += [f"{rec['scenario']}: {n}" for n in rec.get("notes", [])]
    return rows, guarantees, notes


def fmt(x):
    return "—" if x is None else f"{x:.3g}" if abs(x) < 10 else f"{x:.0f}"


def main():
    base_rows, base_g, notes = load(sys.argv[1])
    cand_rows, cand_g, _ = load(sys.argv[2]) if len(sys.argv) > 2 else ({}, {}, [])
    cand = len(sys.argv) > 2
    worse = []

    def section(rows):
        print("| row | better | band | baseline (n) | spread | " + ("candidate (n) | spread | Δ | verdict |" if cand else "steady |"))
        print("|---|---|---|---|---|" + ("---|---|---|---|" if cand else "---|"))
        for row in rows:
            print(line(row))

    def line(row):
        _, direction, band, judged = bar(row)
        b, c = base_rows.get(row, []), cand_rows.get(row, [])
        bm = statistics.median(b) if b else None
        tie = max(0.05 * abs(bm), band) if bm is not None else band
        spread = (max(b) - min(b)) if len(b) > 1 else None
        noisy = spread is not None and spread > tie
        better = {-1: "lower", 1: "higher", 0: "—"}[direction]
        out = f"| `{row}` | {better} | {'±' + fmt(band) if judged else 'unjudged'} | {fmt(bm)} ({len(b)}) | {fmt(spread)} |"
        if not cand:
            return out + f" {'noisy' if noisy and judged else 'yes' if spread is not None else '—'} |"
        cm = statistics.median(c) if c else None
        cspread = (max(c) - min(c)) if len(c) > 1 else None
        delta = cm - bm if cm is not None and bm is not None else None
        if not judged or direction == 0 or delta is None:
            verdict = "reported"
        elif noisy:
            verdict = "can't judge (noisy)"
        elif -direction * delta > tie:
            verdict = "**WORSE**"
            worse.append(row)
        elif direction * delta > tie:
            verdict = "better"
        else:
            verdict = "tie"
        pct = f"{100 * delta / bm:+.0f}%" if delta is not None and bm else fmt(delta)
        return out + f" {fmt(cm)} ({len(c)}) | {fmt(cspread)} | {pct} | {verdict} |"

    every = sorted(set(base_rows) | set(cand_rows))
    first = [r for r in every if any(fnmatch(r, o) for o in OUTCOMES)]
    print("### Outcomes\n")
    section(first)
    print("\n### Every row\n")
    section([r for r in every if r not in first])
    print("\n### Guarantees\n")
    print("| guarantee | baseline | " + ("candidate |" if cand else ""))
    print("|---|---|" + ("---|" if cand else ""))
    broken = []
    for g in sorted(set(base_g) | set(cand_g)):
        b, c = base_g.get(g, []), cand_g.get(g, [])
        said = lambda xs: f"{sum(xs)}/{len(xs)} held" if xs else "—"
        if cand and c and not all(c):
            broken.append(g)
        print(f"| `{g}` | {said(b)} | " + (f"{said(c)} |" if cand else ""))
    if notes:
        print("\nNotes:\n" + "\n".join(f"- {n}" for n in sorted(set(notes))))
    if cand:
        print(f"\nVerdict: {'FAIL' if worse or broken else 'PASS'}" + (f": worse {', '.join(worse)}" if worse else "") + (f"; broken {', '.join(broken)}" if broken else ""))


if __name__ == "__main__":
    main()
