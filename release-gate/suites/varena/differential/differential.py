#!/usr/bin/env python3
"""differential.py <a.json> <b.json>: are two run records' digests one output? Prints PASS or FAIL and why.
What only one side has (an encoder its memo answered) is not compared; nothing compared is a FAIL. Any difference is a
FAIL: a split level is no excuse, since a split that changes the answer is the bug this exists to catch."""
import json
import sys


def calls(record):
    yield record
    yield from record.get('calls') or []


def facts(path):
    """Its digests by call and name, what `vt.checks` did, each split's level and any replay caught wrong."""
    record = json.load(open(path))
    digests, checks, levels = {}, {'uses': 0, 'unmapped_bytes': 0, 'perturbed_bytes': 0}, {}
    mismatched = 0
    for index, call in enumerate(calls(record)):
        measured = call.get('measurements') or {}
        mismatched += len(((measured.get('execution') or {}).get('safeguards') or {}).get('replay_mismatch', ()))
        for row in (measured.get('execution') or {}).get('loops') or []:
            digests |= {f'{index}.{name}': value for name, value in (row.get('digests') or {}).items()}
            for key, value in (row.get('checks') or {}).items():
                checks[key] = max(checks[key], value)

        def walk(value):
            if isinstance(value, dict):
                for key, held in value.items():
                    if key == 'working_splits':
                        levels.update((f'{index}.{row.get("site", "")}', sorted((row.get('levels') or {}).items())) for row in held)
                    walk(held)
            elif isinstance(value, list):
                for held in value:
                    walk(held)
        walk(measured)
    return record.get('status'), digests, checks, levels, mismatched


def main(a, b):
    (first, ours, _, levels, wrong), (second, theirs, checks, others, wronger) = facts(a), facts(b)
    if first != 'completed' or second != 'completed':
        return f'FAIL runs ended {first} and {second}'
    if wrong or wronger:
        return f'FAIL a replay differed from eager: {wrong} and {wronger}'
    shared = sorted(ours.keys() & theirs.keys())
    if not shared:
        return f'FAIL no digest to compare: {sorted(ours)} against {sorted(theirs)}'
    different = [name for name in shared if ours[name] != theirs[name]]
    said = f'{len(shared)} digests, checks {checks}'
    if not different:
        return f'PASS byte-identical: {said}'
    if levels != others:
        sites = levels.keys() | others.keys()
        moved = sum(levels.get(site) != others.get(site) for site in sites)
        return f'FAIL {different} differ, and split levels moved at {moved} of {len(sites)} sites: {said}'
    return f'FAIL {different} differ: {said}'


if __name__ == '__main__':
    print(main(*sys.argv[1:3]))
