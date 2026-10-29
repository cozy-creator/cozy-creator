"""idle-entrypoints.sh's verdicts: `<dir> <memory log> <card bytes>` in, `name<TAB>PASS|FAIL<TAB>why` rows out.
A cell that did not run prints nothing."""
import json
import sys
from pathlib import Path



def record(name):
    path = Path(sys.argv[1]) / f'{name}.json'
    return json.loads(path.read_text()) if path.exists() else None


def gpu_calls(rec):
    """Each GPU call's measurements, in order."""
    rows = [rec.get('measurements') or {}] + [c.get('measurements') or {} for c in rec.get('calls') or []]
    return [m for m in rows if (m.get('execution') or {}).get('gpus')]


def executor(m):
    e = m['execution']
    return tuple(sorted(g['pid'] for g in e['gpus'])), (e.get('executor') or {}).get('started_unix_ms')


def kernels_ms(m):
    """When its GPU work began and ended, machine clock (unknown: the widest window)."""
    gpus = m['execution']['gpus']
    starts, ends = [g['start_us'] for g in gpus if g.get('start_us')], [g['end_us'] for g in gpus if g.get('end_us')]
    return (min(starts) / 1e3 if starts else float('inf')), (max(ends) / 1e3 if ends else 0.0)


def peak(m):
    return max((d.get('peak_bytes') or 0 for d in (m.get('memory') or {}).get('driver') or []), default=0)


def refilled(m):
    """Weight bytes demand-read again within the call: its own blocks paged out and back."""
    again = 0
    for r in ((m.get('memory') or {}).get('stats') or {}).get('varena') or []:
        seen = set()
        for t in (r.get('stats') or {}).get('trace') or []:
            if t.get('origin') == 'store' and not t.get('speculative'):
                key = (t.get('set'), t.get('block'))
                again += (t.get('bytes') or 0) if key in seen else 0
                seen.add(key)
    return again


def host_stall_s(m):
    stages = (m.get('attribution') or {}).get('stages') or {}
    return sum(v.get('total_ms') or 0 for k, v in stages.items() if k.startswith('host')) / 1e3


def offloads():
    for line in Path(sys.argv[2]).read_text(errors='replace').splitlines():
        head, brace, body = line.partition('{')
        try:
            event = json.loads(brace + body)
            at = float(head.split()[-1])
        except (ValueError, IndexError):
            continue
        if event.get('event') == 'offload' and not event.get('failed'):
            yield at * 1e3, event.get('pid')


def row(name, ok, why):
    print(f'{name}\tPASS\t{why}' if ok else f'{name}\tFAIL\t{why}')


def reclaimed(m):
    """Bytes the process's Runtime had reclaimed of its weights by the call's end (cumulative, every GPU)."""
    return sum(((r.get('stats') or {}).get('weight_residency') or {}).get('reclaimed_bytes') or 0
               for r in ((m.get('memory') or {}).get('stats') or {}).get('varena') or [])


def ready_s(rec, m):
    """From the run's acceptance to its GPU call's first kernel."""
    accepted = rec.get('accepted_at_ms') or 0
    return (kernels_ms(m)[0] - accepted) / 1e3 if accepted else float('nan')


def legs(m):
    return ((m['execution'].get('executor') or {}).get('legs_ms')) or {}


card = int(sys.argv[3] or 0)
pairs = (('idle-klein-a', 'idle-klein-b'), ('idle-klein-b', 'idle-klein-a2'), ('idle-klein-a2', 'idle-klein-b2'),
         ('idle-h3-s', 'idle-h3-t'))
recs = {n: record(n) for pair in pairs for n in pair}
for idle, active in pairs:
    if not recs[active]:
        continue
    ri, ra = recs[idle], recs[active]
    ci = ri and ri.get('status') == 'completed' and gpu_calls(ri)
    ca = ra.get('status') == 'completed' and gpu_calls(ra)
    if not (ci and ca):
        row(active, False, f'{idle} {ri and ri.get("status")}, {active} {ra.get("status")}')
        continue
    mi, ma = ci[-1], ca[-1]
    was, now = executor(mi), executor(ma)
    since, until = kernels_ms(mi)[1], kernels_ms(ma)[0]
    moved = any(pid in was[0] and since <= at <= until for at, pid in offloads())
    ok, why = was == now and not moved, f'on {now}, {idle} on {was}; offloaded by TensorD between: {moved}'
    if active.startswith('idle-klein'):
        # One process fills the card (its Runtime evicts the idle construction's blocks as it needs the room): sitting
        # near the limit is no stall by itself; its own weights read twice would be.
        top, again = peak(ma), refilled(ma)
        ok = ok and again == 0
        why += f'; driver peak {top / 2**30:.2f} of {card / 2**30:.2f} GiB; {again} B of its weights read again'
    evicted = reclaimed(ma) - reclaimed(mi) if was == now else reclaimed(ma)
    row(active, ok, f'{why}; Runtime reclaimed {evicted / 2**30:.2f} GiB of weights during it; ready '
        f'{ready_s(ra, ma):.1f} s after acceptance ({idle} {ready_s(ri, mi):.1f} s); host stall {host_stall_s(ma):.1f} s')
t, lf = record('idle-h3-t'), record('idle-h3-lf')
if lf:
    ok = all(r is not None and r.get('status') == 'completed' and gpu_calls(r) for r in (t, lf))
    first, second = (executor(gpu_calls(r)[-1]) if ok else None for r in (t, lf))
    row('idle-h3-lf', ok and first == second,
        f'ref2va_turbo executor {first}, long_form segment executor {second}; statuses '
        f'{t and t.get("status")}, {lf and lf.get("status")}')
# A cancel ends every construction of its process; the next call of a sibling starts a new one, cold.
for befores, canceled, after in ((('idle-klein-b2',), 'idle-cancel', 'idle-after-cancel'),
                                 (('idle-h3-lf', 'idle-h3-t'), 'idle-h3-cancel', 'idle-h3-after-cancel')):
    rc, ra = record(canceled), record(after)
    if not ra:
        continue
    rb = next(filter(None, map(record, befores)), None)
    ca = ra.get('status') == 'completed' and gpu_calls(ra)
    cb = rb and rb.get('status') == 'completed' and gpu_calls(rb)
    if not (rc and rc.get('status') == 'canceled' and ca and cb):
        row(after, False, f'{canceled} {rc and rc.get("status")}, {after} {ra.get("status")}')
        continue
    was, now = executor(cb[-1]), executor(ca[-1])
    row(after, was != now, f'on {now} after {canceled} was canceled, which ran beside {was}; ready '
        f'{ready_s(ra, ca[-1]):.1f} s after acceptance (warm before: {ready_s(rb, cb[-1]):.1f} s), its start {legs(ca[-1])}')
