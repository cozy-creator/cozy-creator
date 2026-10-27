"""idle-entrypoints.sh's verdicts: `<dir> <memory log> <card bytes>` in, `name<TAB>PASS|FAIL<TAB>why` rows out.
A pair none of whose cells ran prints nothing."""
import json
import sys
from pathlib import Path

LIMIT = 512 << 20  # the driver's reserve: a call within it of the card sat at its limit


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


card = int(sys.argv[3] or 0)
klein = {n: record(f'idle-klein-{n}') for n in ('a', 'b', 'a2', 'b2')}
if any(klein.values()):
    calls = {n: r and r.get('status') == 'completed' and gpu_calls(r) for n, r in klein.items()}
    for idle, active in (('a', 'b'), ('a2', 'b2')):
        if not (calls[idle] and calls[active]):
            row(f'idle-klein-{active}', False, f'{idle} {klein[idle] and klein[idle].get("status")}, '
                f'{active} {klein[active] and klein[active].get("status")}')
            continue
        mi, ma = calls[idle][-1], calls[active][-1]
        pids, _ = executor(mi)
        since, until = kernels_ms(mi)[1], kernels_ms(ma)[0]
        moved = any(pid in pids and since <= at <= until for at, pid in offloads())
        top, again = peak(ma), refilled(ma)
        row(f'idle-klein-{active}', moved and card and top < card - LIMIT and again == 0,
            f'idle executor {pids} offloaded first: {moved}; driver peak {top / 2**30:.2f} of {card / 2**30:.2f} GiB; '
            f'{again} B of its weights read again; host stall {host_stall_s(ma):.1f} s')
    for first, again in (('a', 'a2'), ('b', 'b2')):
        if calls[first] and calls[again]:
            was, now = executor(calls[first][-1]), executor(calls[again][-1])
            row(f'idle-klein-{again}-executor', was == now, f'{again} on executor {now}, {first} on {was}')
t, lf = record('idle-h3-t'), record('idle-h3-lf')
if t or lf:
    ok = all(r is not None and r.get('status') == 'completed' and gpu_calls(r) for r in (t, lf))
    first, second = (executor(gpu_calls(r)[-1]) if ok else None for r in (t, lf))
    row('idle-h3-lf', ok and first == second,
        f'ref2va_turbo executor {first}, long_form segment executor {second}; statuses '
        f'{t and t.get("status")}, {lf and lf.get("status")}')
