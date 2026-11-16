"""tfs-300: one scenario's arms (e2e/cli/link.sh) into its record, on stdout.

  judge.py <scenario> <dir>     <dir>/<arm>/: meta.json, kernel.ndjson, status*.ndjson, probe.ndjson, <run>.json, <run>.stamped

Rates are the kernel's (iptables bytes per class between samples); times are the run records' and the asks' (the runner's clock);
per-model bytes are Status's. A row is a number; table.py holds each row's direction and tie band. `guarantees` are the yes/no
ones; `failed` names each that didn't hold."""

import json, re, statistics, sys
from pathlib import Path

MIB = 1 << 20


def ndjson(path):
    rows = []
    if path.exists():
        for line in path.read_text(errors="replace").splitlines():
            try:
                rows.append(json.loads(line))
            except ValueError:
                pass
    return rows


def pct(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else None


class Arm:
    def __init__(self, d: Path):
        self.d, self.meta = d, json.loads((d / "meta.json").read_text())
        self.kernel = ndjson(d / "kernel.ndjson")
        self.status = {p.stem: ndjson(p) for p in d.glob("status*.ndjson")}
        self.probe = ndjson(d / "probe.ndjson")
        self.classes = list(self.meta.get("classes", {}))
        k = self.kernel
        # (t_end, seconds, {class: bytes/s}, total bytes/s) between consecutive samples
        self.rates = []
        for a, b in zip(k, k[1:]):
            dt = (b["t"] - a["t"]) / 1e3
            if dt <= 0:
                continue
            r = {c: max(0, b["bytes"].get(c, 0) - a["bytes"].get(c, 0)) / dt for c in self.classes}
            self.rates.append((b["t"], dt, r, sum(r.values())))
        self.ceiling, self.ceiling_kind = self._ceiling()

    def _ceiling(self):
        m = self.meta
        if "ceiling_to_ms" in m:
            lo, hi = m["ceiling_from_ms"] + 2000, m["ceiling_to_ms"] - 1000
            got = [(dt, r["files"]) for t, dt, r, _ in self.rates if lo < t <= hi]
            secs = sum(dt for dt, _ in got)
            if secs > 3:
                return sum(dt * x for dt, x in got) / secs, "measured"
        # Open: the best 5 s of the arm.
        best, window = 0.0, []
        for t, dt, _, total in self.rates:
            window = [w for w in window + [(t, dt, total)] if w[0] > t - 5000]
            secs = sum(w[1] for w in window)
            if secs >= 4:
                best = max(best, sum(w[1] * w[2] for w in window) / secs)
        return best or 1.0, "observed"

    def at(self, key):
        return self.meta.get(key)

    def run(self, name):
        p = self.d / f"{name}.json"
        return json.loads(p.read_text()) if p.exists() else {}

    def series(self, cls, lo, hi):
        return [(t, dt, r[cls], total) for t, dt, r, total in self.rates if lo < t <= hi]

    def moved(self, cls, lo, hi):
        return sum(dt * x for _, dt, x, _ in self.series(cls, lo, hi))

    def until(self, cls, lo, need):
        """When the class had moved `need` bytes since `lo`."""
        got = 0.0
        for t, dt, r, _ in self.rates:
            if t > lo:
                got += dt * r[cls]
                if got >= need:
                    return t
        return None

    def longest(self, cls, lo, hi, also=None):
        """Longest stretch (s) the class moved under 1% of the ceiling between lo and hi (and, with `also`, the link carried
        under that share of it)."""
        best = run = 0.0
        for _, dt, x, total in self.series(cls, lo, hi):
            low = x < 0.01 * self.ceiling and (also is None or total < also * self.ceiling)
            run = run + dt if low else 0.0
            best = max(best, run)
        return round(best, 1)

    def use(self, spans):
        """Share of the seconds anything was wanted that the link ran at 90% of its ceiling or more; seconds under half of it."""
        full = idle = wanted = 0.0
        for t, dt, _, total in self.rates:
            if any(lo < t <= hi for lo, hi in spans):
                wanted += dt
                full += dt * (total >= 0.9 * self.ceiling)
                idle += dt * (total < 0.5 * self.ceiling)
        return (round(full / wanted, 3) if wanted else None), round(idle, 1)

    def queue_ms(self, lo, hi):
        rows = [r for r in self.kernel if lo < r["t"] <= hi and r.get("q")]
        delays = [r["q"].get("backlog", 0) / self.ceiling * 1e3 for r in rows]
        drops = (rows[-1]["q"].get("drops", 0) - rows[0]["q"].get("drops", 0)) if len(rows) > 1 else None
        return pct(delays, 0.5), pct(delays, 0.95), (max(delays) if delays else None), drops

    def ops(self, kind, match, poller=None):
        """[(t, bytes_done, bytes_total)] of Status's operations of `kind` whose subject contains `match`."""
        rows = []
        for name, polled in self.status.items():
            if poller and name != poller:
                continue
            for row in polled:
                for op in row.get("ops") or []:
                    if op.get("kind") == kind and match in (op.get("subject") or ""):
                        rows.append((row["t"], op.get("bytes_done") or 0, op.get("bytes_total") or 0))
        return sorted(rows)

    def status_ms(self, lo, hi):
        return [r["ms"] for rows in self.status.values() for r in rows if lo < r["t"] <= hi and r.get("ok")]

    def stamped(self, name, pattern):
        """The runner's times of a run's event lines matching `pattern`."""
        return [r["t"] for r in ndjson(self.d / f"{name}.stamped") if re.search(pattern, r.get("line", ""))]

    def log(self):
        p = self.d / "tensord.log"
        text = p.read_text(errors="replace") if p.exists() else ""
        lo = self.at("began_ms") or 0
        lines = [l for l in text.splitlines() if stamp_ms(l) >= lo]
        settled = [int(m[1]) for l in lines for m in [re.search(r"streams settled\S*\s+streams=(\d+)", l)] if m]
        return {"fetching": sum(": fetching " in l for l in lines), "landed": sum(": landed " in l for l in lines),
                "refused": sum("fetch refused" in l for l in lines), "non_2xx": sum(bool(re.search(r"\b(status|answered)[= ]+[3-5]\d\d\b", l)) for l in lines),
                "hedged": sum(int(m[1]) for l in lines for m in [re.search(r"hedged=(\d+)", l)] if m),
                "streams_settled": settled[-1] if settled else None}

    def landed(self):
        """TensorD's own word of each object landed: [(t, job, bytes)], job = its order's high 32 bits (a run's job order)."""
        p = self.d / "tensord.log"
        out = []
        for line in (p.read_text(errors="replace") if p.exists() else "").splitlines():
            found = re.search(r": landed .*bytes=(\d+).*priority=(\d+)", line)
            if found and stamp_ms(line) >= (self.at("began_ms") or 0):
                out.append((stamp_ms(line), int(found[2]) >> 32, int(found[1])))
        return out

    def facts(self):
        p50, p95, top, drops = self.queue_ms(self.at("began_ms") or 0, self.at("ended_ms") or 1 << 62)
        return {"form": self.at("form"), "cc": self.at("cc"), "mtu": self.at("mtu"), "ceiling_mb_s": round(self.ceiling / 1e6, 2),
                "ceiling": self.ceiling_kind, "queue_ms": {"p50": p50, "p95": p95, "max": top}, "drops": drops, "log": self.log()}


def stamp_ms(line):
    from datetime import datetime
    try:
        return int(datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        return 0


def settled(arm, name):
    return arm.run(name).get("settled_at_ms")


def completed(arm, *names):
    return all(arm.run(n).get("status") == "completed" for n in names)


def secs(a, b):
    return round((b - a) / 1e3, 1) if a is not None and b is not None else None


def ratio(a, b):
    return round(a / b, 3) if a is not None and b else None


class Record:
    def __init__(self, scenario, arms):
        self.scenario, self.arms = scenario, arms
        self.rows, self.guarantees, self.failed, self.notes = {}, {}, [], []

    def row(self, name, value):
        self.rows[f"{self.scenario}.{name}"] = value

    def hold(self, name, ok, why):
        self.guarantees[f"{self.scenario}.{name}"] = bool(ok)
        if not ok:
            self.failed.append(f"{name}: {why}")

    def done(self):
        first = next(iter(self.arms.values()), None)
        return {"schema": "link-metrics/1", "scenario": self.scenario, "rows": self.rows, "guarantees": self.guarantees,
                "failed": self.failed, "notes": self.notes, "runner": first.at("runner") if first else None,
                "arms": {name: arm.facts() for name, arm in self.arms.items()}}


def total_of(arm, subject, mib):
    totals = [total for _, _, total in arm.ops("download", subject)]
    return max(totals) if totals else mib * MIB


def weights_done(arm, ask, subject, mib):
    """When a model's bytes had all come: Status's total for it (else `mib`), counted from the ask on the weights class."""
    totals = [total for _, _, total in arm.ops("download", subject)]
    need = (max(totals) if totals else mib * MIB) * 0.99
    return arm.until("weights", ask, need), need


def install_span(arm, name, package):
    """The install's first and last second, as Status listed it, else as the run's events said it."""
    seen = [t for t, _, _ in arm.ops("install", package)]
    if seen:
        return min(seen), max(seen) + 1000, "status"
    said = arm.stamped(name, r"installing")
    after = arm.stamped(name, r".")
    if said:
        end = next((t for t in after if t > max(said)), max(said))
        return min(said), end, "events"
    return None, None, "none"


def cold(rec):
    for form in ("q60", "p60", "open"):
        i, w, c = (rec.arms.get(f"{form}-{k}") for k in ("install", "weights", "cold"))
        if not (i and w and c):
            continue
        ok = completed(i, "install") and completed(w, "weights") and completed(c, "cold")
        rec.hold(f"{form}.completed", ok, f"{form}: a run didn't complete")
        ask_i, ask_w, ask_c = i.at("ask_install"), w.at("ask_weights"), c.at("ask_cold")
        i0, i1, src_a = install_span(i, "install", f"e2e-link-{form}a")
        c0, c1, src_c = install_span(c, "cold", f"e2e-link-{form}c")
        rec.notes.append(f"{form}: install spans from {src_a} / {src_c}")
        alone_w = secs(ask_w, settled(w, "weights"))
        done, need = weights_done(c, ask_c, "pagebox", 1600)
        rec.row(f"{form}.ttfo_s", secs(c.run("cold").get("accepted_at_ms") or ask_c, settled(c, "cold")))
        rec.row(f"{form}.install_alone_s", secs(i0, i1))
        rec.row(f"{form}.install_s", secs(c0, c1))
        rec.row(f"{form}.install_ratio", ratio(secs(c0, c1), secs(i0, i1)))
        rec.row(f"{form}.weights_alone_s", alone_w)
        rec.row(f"{form}.weights_s", secs(ask_c, done))
        rec.row(f"{form}.weights_ratio", ratio(secs(ask_c, done), alone_w))
        rec.row(f"{form}.weights_vs_ceiling", ideal(c, need / 0.99, ask_c, done))
        end = done or settled(c, "cold") or c.at("ended_ms")
        rec.row(f"{form}.weights_longest_zero_s", c.longest("weights", ask_c, end))
        starved = c.longest("weights", ask_c, end, also=0.5)
        rec.hold(f"{form}.no_starvation", starved < 5, f"{form}: the weights sat under 1% of the link for {starved} s while it carried under half")
        use, idle = c.use([(ask_c, end)])
        rec.row(f"{form}.link_use", use)
        rec.row(f"{form}.idle_wanted_s", idle)
        # The install downloading: a second its files move a tenth of the link; quiet: none, two seconds from any busy one.
        if c0 and c1:
            span = [(t, dt, r["files"], r["weights"]) for t, dt, r, _ in c.rates if c0 < t <= min(c1, end)]
            busy = [t for t, _, f, _ in span if f > 0.1 * c.ceiling]
            quiet = [(dt, w) for t, dt, f, w in span if f < 0.01 * c.ceiling and all(abs(t - b) > 2000 for b in busy)]
            during = [(dt, w) for t, dt, f, w in span if t in busy]
            share = lambda xs: round(sum(dt * w for dt, w in xs) / sum(dt for dt, _ in xs) / c.ceiling, 3)
            qs = sum(dt for dt, _ in quiet)
            rec.row(f"{form}.install_quiet_s", round(qs, 1))
            rec.row(f"{form}.weights_quiet_share", share(quiet) if qs >= 5 else None)
            rec.row(f"{form}.weights_install_share", share(during) if during else None)
            rec.row(f"{form}.install_rate_ratio", ratio(rate_busy(c, c0, c1), rate_busy(i, i0, i1)))
        p50, p95, top, drops = c.queue_ms(ask_c, end)
        rec.row(f"{form}.queue_p95_ms", round(p95) if p95 is not None else None)


def ideal(arm, nbytes, ask, done):
    """The model's bytes at the link's ceiling over the time they took: 1.0 is the link full from the ask to the last byte."""
    return round(nbytes / arm.ceiling / ((done - ask) / 1e3), 3) if done and ask and done > ask else None


def rate_busy(arm, lo, hi):
    if lo is None:
        return None
    xs = [(dt, x) for _, dt, x, _ in arm.series("files", lo, hi) if x > 0.1 * arm.ceiling]
    s = sum(dt for dt, _ in xs)
    return sum(dt * x for dt, x in xs) / s if s else None


def subject_rate(arm, subject, lo, hi, poller=None):
    """Status's bytes of one model between polls: [(t, dt, bytes/s)]."""
    pts = [(t, d) for t, d, _ in arm.ops("download", subject, poller) if lo - 2000 <= t <= hi + 2000]
    return [(b[0], (b[0] - a[0]) / 1e3, max(0, b[1] - a[1]) / ((b[0] - a[0]) / 1e3)) for a, b in zip(pts, pts[1:]) if b[0] > a[0]]


def longest_subject(arm, subject, lo, hi, poller=None):
    best = run = 0.0
    for _, dt, x in subject_rate(arm, subject, lo, hi, poller):
        run = run + dt if x < 0.01 * arm.ceiling else 0.0
        best = max(best, run)
    return round(best, 1)


def need(rec):
    a, b, both = rec.arms.get("need-alone"), rec.arms.get("ahead-alone"), rec.arms.get("both")
    if not (a and b and both):
        return
    rec.hold("completed", completed(a, "need") and completed(b, "ahead") and completed(both, "need", "ahead"), "a run didn't complete")
    read = lambda arm: (arm.run("need").get("result") or {}).get("read_s")
    rec.row("read_alone_s", round(read(a), 1) if read(a) is not None else None)
    rec.row("read_s", round(read(both), 1) if read(both) is not None else None)
    rec.row("read_ratio", ratio(read(both), read(a)))
    alone = secs(b.at("ask_ahead"), settled(b, "ahead"))
    ask, done = both.at("ask_ahead"), settled(both, "ahead")
    rec.row("ahead_alone_s", alone)
    rec.row("ahead_s", secs(ask, done))
    rec.row("ahead_ratio", ratio(secs(ask, done), alone))
    rec.row("ahead_vs_ceiling", ideal(both, total_of(both, "pagebox", 1600), ask, done))
    use, idle = both.use([(both.at("ask_need"), max(done or 0, settled(both, "need") or 0))])
    rec.row("link_use", use)
    rec.row("idle_wanted_s", idle)
    rec.row("ahead_longest_zero_s", longest_subject(both, "pagebox", ask, done or ask))
    starved = both.longest("weights", ask, done or ask, also=0.5)
    rec.hold("no_starvation", starved < 5, f"the link sat under half with the weights under 1% for {starved} s")


def thirdparty(rec):
    a, b, both = rec.arms.get("fetch-alone"), rec.arms.get("ahead-alone"), rec.arms.get("both")
    if not (a and b and both):
        return
    rec.hold("completed", completed(a, "fetch") and completed(b, "ahead") and completed(both, "fetch", "ahead"), "a run didn't complete")
    took = lambda arm: (arm.run("fetch").get("result") or {}).get("read_s")
    rec.row("fetch_alone_s", round(took(a), 1) if took(a) is not None else None)
    rec.row("fetch_s", round(took(both), 1) if took(both) is not None else None)
    rec.row("fetch_ratio", ratio(took(both), took(a)))
    alone = secs(b.at("ask_ahead"), settled(b, "ahead"))
    ask, done = both.at("ask_ahead"), settled(both, "ahead")
    rec.row("ahead_alone_s", alone)
    rec.row("ahead_s", secs(ask, done))
    rec.row("ahead_ratio", ratio(secs(ask, done), alone))
    rec.row("ahead_vs_ceiling", ideal(both, total_of(both, "pagebox", 1600), ask, done))
    f0 = (both.run("fetch").get("result") or {}).get("began_unix_ms")
    if f0 and took(both):
        f1 = f0 + took(both) * 1e3
        rec.row("ahead_share_during_fetch", round(both.moved("weights", f0, f1) / ((f1 - f0) / 1e3) / both.ceiling, 3))
        rec.row("fetch_share", round(both.moved("files", f0, f1) / ((f1 - f0) / 1e3) / both.ceiling, 3))
    use, idle = both.use([(ask, done or ask)])
    rec.row("link_use", use)
    starved = both.longest("weights", ask, done or ask, also=0.5)
    rec.hold("no_starvation", starved < 5, f"the link sat under half with the weights under 1% for {starved} s")


def cancel(rec):
    for form, arm in rec.arms.items():
        c0, c1, end = arm.at("cancel_ms"), arm.at("canceled_ms"), arm.at("ended_ms")
        if not c0:
            continue
        status = arm.run("read").get("status")
        rec.hold(f"{form}.canceled", status == "canceled", f"{form}: the run ended {status}, not canceled")
        # Released: the first time after the cancel from which every 0.5 s of the weights is under 1% of the link to the end.
        ser = arm.series("weights", c0, end)
        last_busy = c0
        window = []
        for t, dt, x, _ in ser:
            window = [w for w in window + [(t, dt, x)] if w[0] > t - 500]
            s = sum(w[1] for w in window)
            if s and sum(w[1] * w[2] for w in window) / s >= 0.01 * arm.ceiling:
                last_busy = t
        released = secs(c0, last_busy)
        rec.row(f"{form}.release_s", released)
        rec.row(f"{form}.cancel_call_s", secs(c0, c1))
        rec.row(f"{form}.bytes_after_mib", round(arm.moved("weights", c0, end) / MIB, 1))
        rec.hold(f"{form}.stops", released is not None and released <= 10 and end - last_busy >= 10000,
                 f"{form}: the weights still moved {released} s after the cancel")


def update(rec):
    w, u, both = rec.arms.get("weights-alone"), rec.arms.get("update-alone"), rec.arms.get("both")
    if not (w and u and both):
        return
    rec.hold("completed", completed(w, "weights") and completed(both, "weights"), "a run didn't complete")
    rec.row("update_alone_s", secs(u.at("update_ms"), u.at("updated_ms")))
    rec.row("update_s", secs(both.at("update_ms"), both.at("updated_ms")))
    alone = secs(w.at("ask_weights"), settled(w, "weights"))
    ask, done = both.at("ask_weights"), settled(both, "weights")
    rec.row("weights_alone_s", alone)
    rec.row("weights_s", secs(ask, done))
    rec.row("weights_ratio", ratio(secs(ask, done), alone))
    rec.row("weights_vs_ceiling", ideal(both, total_of(both, "pagebox", 1600), ask, done))
    rec.row("weights_longest_zero_s", both.longest("weights", ask, done or ask))
    rec.row("goodput", ratio(both.moved("weights", ask, done or ask), w.moved("weights", w.at("ask_weights"), settled(w, "weights") or 0)))
    rec.notes.append(f"TensorD {both.at('tensord_before')} → {both.at('tensord_after')} (restarted: {both.at('tensord_before') != both.at('tensord_after')})")
    starved = both.longest("weights", ask, done or ask, also=0.5)
    rec.hold("no_starvation", starved < 5, f"the link sat under half with the weights under 1% for {starved} s")


def control(rec):
    for form, arm in rec.arms.items():
        b0, idle_to = arm.at("began_ms"), arm.at("idle_to_ms")
        ask, done = arm.at("ask_weights"), settled(arm, "weights")
        if not (idle_to and ask):
            continue
        rec.hold(f"{form}.completed", completed(arm, "weights"), f"{form}: the download didn't complete")
        full = [t for t, _, x, _ in arm.series("weights", ask, done or ask) if x >= 0.5 * arm.ceiling]
        lo, hi = (min(full), max(full)) if full else (ask, done or ask)
        idle = [p["ms"] for p in arm.probe if b0 < p["t"] <= idle_to and "code" in p]
        busy = [p["ms"] for p in arm.probe if lo < p["t"] <= hi and "code" in p]
        failed = [p for p in arm.probe if "error" in p]
        rec.row(f"{form}.probe_idle_p95_ms", pct(idle, 0.95))
        rec.row(f"{form}.probe_p50_ms", pct(busy, 0.5))
        rec.row(f"{form}.probe_p95_ms", pct(busy, 0.95))
        rec.row(f"{form}.probe_p99_ms", pct(busy, 0.99))
        rec.row(f"{form}.probe_max_ms", max(busy) if busy else None)
        rec.row(f"{form}.probe_p95_over_idle_ms", pct(busy, 0.95) - pct(idle, 0.95) if busy and idle else None)
        rec.row(f"{form}.status_idle_p95_ms", pct(arm.status_ms(b0, idle_to), 0.95))
        rec.row(f"{form}.status_p95_ms", pct(arm.status_ms(lo, hi), 0.95))
        rec.row(f"{form}.probe_failures", len(failed))
        rec.row(f"{form}.weights_s", secs(ask, done))
        rec.row(f"{form}.weights_vs_ceiling", ideal(arm, total_of(arm, "pagebox", 1600), ask, done))
        p50, p95, top, drops = arm.queue_ms(lo, hi)
        rec.row(f"{form}.queue_p95_ms", round(p95) if p95 is not None else None)
        rec.hold(f"{form}.control_never_failed", not failed, f"{form}: {len(failed)} control calls failed, first: {failed[0]['error'] if failed else ''}")
        if not busy:
            rec.notes.append(f"{form}: the weights never reached half the link: no saturated window")


def two(rec):
    a, b, both = rec.arms.get("a-alone"), rec.arms.get("b-alone"), rec.arms.get("both")
    if not (a and b and both):
        return
    rec.hold("completed", completed(a, "a") and completed(b, "b") and completed(both, "a", "b"), "a run didn't complete")
    alone = {"a": secs(a.at("ask_a"), settled(a, "a")), "b": secs(b.at("ask_b"), settled(b, "b"))}
    took = {k: secs(both.at(f"ask_{k}"), settled(both, k)) for k in "ab"}
    for k in "ab":
        rec.row(f"{k}_alone_s", alone[k])
        rec.row(f"{k}_s", took[k])
    rec.row("both_s", secs(min(both.at("ask_a"), both.at("ask_b")), max(settled(both, "a") or 0, settled(both, "b") or 0) or None))
    r = [ratio(took[k], alone[k]) for k in "ab"]
    if None not in r:
        rec.row("mean_ratio", round(sum(r) / 2, 3))
        rec.row("worst_ratio", max(r))
    # Contended: both asked, neither done. Each project's bytes as TensorD's log lands them (an object at a time: Status lists a
    # project's downloads only to a page that names the project, not to `machine show`): the earlier job is a's.
    lo, hi = max(both.at("ask_a"), both.at("ask_b")), min(settled(both, "a") or 0, settled(both, "b") or 0)
    jobs = sorted({j for _, j, _ in both.landed() if j})
    if hi > lo and len(jobs) >= 2:
        pa = [(t, b) for t, j, b in both.landed() if j == jobs[0]]
        pb = [(t, b) for t, j, b in both.landed() if j == jobs[1]]
        upto = lambda pts, t: sum(b for s, b in pts if lo < s <= t)
        da, db = upto(pa, hi), upto(pb, hi)
        if da + db:
            rec.row("share_a", round(da / (da + db), 3))
            rec.row("jain", round((da + db) ** 2 / (2 * (da * da + db * db)), 3))
            gaps = [abs(upto(pa, t) - upto(pb, t)) / (upto(pa, t) + upto(pb, t)) for t, _ in sorted(pa + pb) if lo < t <= hi and upto(pa, t) + upto(pb, t)]
            rec.row("progress_gap", round(max(gaps), 3) if gaps else None)
        rec.notes.append(f"projects' bytes from TensorD's landed lines, jobs {jobs[:2]}")
    else:
        rec.notes.append(f"no contended window with both projects' landed bytes (jobs {jobs})")
    use, idle = both.use([(both.at("ask_a"), max(settled(both, "a") or 0, settled(both, "b") or 0))])
    rec.row("link_use", use)


def disk(rec):
    for name, arm in rec.arms.items():
        ask, done = arm.at("ask_weights"), settled(arm, "weights")
        if not ask:
            continue
        rec.hold(f"{name}.completed", completed(arm, "weights"), f"{name}: the download didn't complete")
        rows = [r for r in arm.kernel if ask <= r["t"] <= (done or ask)]
        if len(rows) < 2:
            continue
        r0 = rows[0]
        anon = [r["tensord"]["anon_kib"] for r in rows if (r.get("tensord") or {}).get("anon_kib")]
        wire = lambda r: r["bytes"].get("weights", 0) - r0["bytes"].get("weights", 0)
        disk_ = lambda r: (r.get("disk") or {}).get("written", 0) - (r0.get("disk") or {}).get("written", 0)
        wrote = lambda r: (r.get("tensord") or {}).get("wchar", 0) - (r0.get("tensord") or {}).get("wchar", 0)
        same = [r for r in rows if (r.get("tensord") or {}).get("pid") == (r0.get("tensord") or {}).get("pid")]
        secs_ = (rows[-1]["t"] - r0["t"]) / 1e3
        rec.row(f"{name}.weights_s", secs(ask, done))
        rec.row(f"{name}.disk_mb_s", round(disk_(rows[-1]) / secs_ / 1e6, 1))
        rec.row(f"{name}.link_mb_s", round(wire(rows[-1]) / secs_ / 1e6, 1))
        rec.row(f"{name}.link_to_disk", ratio(wire(rows[-1]), disk_(rows[-1])))
        rec.row(f"{name}.fetched_unwritten_max_mib", round(max(wire(r) - disk_(r) for r in rows) / MIB))
        rec.row(f"{name}.unwritten_in_tensord_max_mib", round(max(wire(r) - wrote(r) for r in same) / MIB))
        rec.row(f"{name}.anon_growth_mib", round((max(anon) - anon[0]) / 1024) if anon else None)
        cap = arm.at("disk_cap_mb_s")
        if cap:
            rec.row(f"{name}.disk_to_cap", round(disk_(rows[-1]) / secs_ / 1e6 / cap, 3))
            rec.notes.append(f"{name}: io.max {arm.at('io_max')}")


def main():
    scenario, d = sys.argv[1], Path(sys.argv[2])
    arms = {p.name: Arm(p) for p in sorted(d.iterdir()) if (p / "meta.json").is_file() and (p / "kernel.ndjson").is_file()}
    rec = Record(scenario, arms)
    {"cold": cold, "need": need, "thirdparty": thirdparty, "cancel": cancel, "update": update, "control": control, "two": two,
     "disk": disk}[scenario](rec)
    print(json.dumps(rec.done(), indent=1))


if __name__ == "__main__":
    main()
