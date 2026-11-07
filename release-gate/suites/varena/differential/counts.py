#!/usr/bin/env python3
"""counts.py <arm> <call 1.json> <call 2.json> <call 3.json>: did every write outside the loop's tensors run exactly as
often as running without graphs would, and did the calls take the path the arm is for? Prints PASS or FAIL and why."""
import json
import sys


def rows(record, key):
    found = []

    def walk(value):
        if isinstance(value, dict):
            if key in value:
                found.append(value)
            for held in value.values():
                walk(held)
        elif isinstance(value, list):
            for held in value:
                walk(held)
    walk(record)
    return found


def main(arm, *paths):
    wrong, seen = [], []
    for call, path in enumerate(paths, 1):
        record = json.load(open(path))
        if record.get('status') != 'completed':
            return f'FAIL call {call} ended {record.get("status")}'
        counted = rows(record, 'writes')
        loops = [row for row in rows(record, 'native_steps') if 'mode' in row]
        if not counted or not loops:
            return f'FAIL call {call}: no counts or no loop row'
        writes, want = counted[-1]['writes'], counted[-1]['want']
        wrong += [f'call {call} {name} {n} of {want[name.split("_")[0]]}' for name, n in sorted(writes.items())
                  if n != want[name.split('_')[0]]]
        row = loops[0]
        seen.append(f'{row.get("mode")}{"+reused" if row.get("reused") else ""}'
                    f'{"+recorded" if row.get("whole_call") else ""}{"+idle" if row.get("recorded_idle_ms") else ""}')
    path = {'writes': ['resident+recorded', 'resident+reused', 'resident+reused'],
            'writes-idle': ['eager', 'resident+reused+idle', 'resident+reused+idle']}[arm]
    if seen != path:
        return f'FAIL the calls ran {seen}, not {path}'
    return f'FAIL {", ".join(wrong)}' if wrong else f'PASS exact over {seen}: {writes}'


if __name__ == '__main__':
    print(main(*sys.argv[1:]))
