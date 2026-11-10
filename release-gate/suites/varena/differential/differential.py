#!/usr/bin/env python3
"""differential.py <a.json> <b.json> [check] [why]: are two run records one output? Prints PASS or FAIL and why.
What only one side has (an encoder its memo answered) is not compared; nothing compared is a FAIL. At the same split
levels the bytes must match. Where levels moved with the room, or `why` names another cause the bytes follow (Paul,
11-06: determinism is desirable, not required), the outputs are judged by quality instead: their 32 px PSNR, neither
flat, and `<check>.jpg` side by side for an eye."""
import json
import math
import os
import subprocess
import sys
from collections.abc import Iterator
from typing import TypedDict

# RGB PSNR at 32 px on the 0.21.0 5090 gate: renders whose levels moved 16.3 (H3, 30 steps) to 69 dB (SDXL); unrelated
# renders 8.8-12.2 dB; a render against grey or noise 14.7, which the flatness bar takes.
FLOOR = 14.0
FLAT = 8.0  # a thumbnail's std: renders 37-60, grey 0, noise averaged to a thumbnail 2.6


# The parts of `cozy run show <n> --json --full` read here.
class Loop(TypedDict, total=False):  # execution.loops[]: hook.py's row
    digests: dict[str, str]
    checks: dict[str, int]


class Execution(TypedDict, total=False):
    safeguards: dict[str, list[str]]
    loops: list[Loop]


class Measurements(TypedDict, total=False):
    execution: Execution


class Call(TypedDict, total=False):
    measurements: Measurements


class Output(TypedDict, total=False):
    media_type: str
    path: str


class RunRecord(Call, total=False):
    status: str
    calls: list[Call]
    output: list[Output]


def calls(record: RunRecord) -> Iterator[Call]:
    yield record
    yield from record.get('calls') or []


def facts(path: str) -> tuple[RunRecord, dict[str, str], dict[str, int], dict[str, frozenset[str]], int]:
    """Its digests by call and name, what `vt.checks` did, each split site's levels and any replay caught wrong."""
    record: RunRecord = json.load(open(path))
    digests: dict[str, str] = {}
    checks = {'uses': 0, 'unmapped_bytes': 0, 'perturbed_bytes': 0}
    levels: dict[str, frozenset[str]] = {}
    mismatched = 0
    for index, call in enumerate(calls(record)):
        measured = call.get('measurements') or {}
        mismatched += len(((measured.get('execution') or {}).get('safeguards') or {}).get('replay_mismatch', ()))
        for row in (measured.get('execution') or {}).get('loops') or []:
            digests |= {f'{index}.{name}': value for name, value in (row.get('digests') or {}).items()}
            for key, value in (row.get('checks') or {}).items():
                checks[key] = max(checks[key], value)

        def walk(value: object) -> None:
            if isinstance(value, dict):
                for key, held in value.items():
                    if key == 'working_splits':
                        levels.update((f'{index}.{row.get("site", "")}', frozenset(row.get('levels') or ())) for row in held)
                    walk(held)
            elif isinstance(value, list):
                for held in value:
                    walk(held)
        walk(measured)
    return record, digests, checks, levels, mismatched


def output(record: RunRecord, path: str) -> str | None:
    """The record's first image or video, where the gate landed it (a copied gate keeps it under project/files)."""
    for row in record.get('output') or []:
        if (row.get('media_type') or '').split('/')[0] in ('image', 'video') and row.get('path'):
            here = os.path.join(os.path.dirname(os.path.abspath(path)), 'project', 'files', os.path.basename(row['path']))
            return row['path'] if os.path.exists(row['path']) else here
    return None


def thumbnail(path: str) -> bytes:
    """Every frame at 32 px wide, RGB bytes."""
    return subprocess.run(['ffmpeg', '-nostdin', '-loglevel', 'error', '-i', path, '-vf', 'scale=32:-2:flags=area', '-f',
                           'rawvideo', '-pix_fmt', 'rgb24', '-'], capture_output=True, check=True).stdout


def std(data: bytes) -> float:
    mean = sum(data) / len(data)
    return math.sqrt(sum((x - mean) ** 2 for x in data) / len(data))


def quality(a: str | None, b: str | None, check: str) -> tuple[float, float] | None:
    """(PSNR, flattest std) of two outputs' thumbnails, `check`.jpg beside them; None with no outputs to judge."""
    if not a or not b or not os.path.exists(a) or not os.path.exists(b):
        return None
    ours, theirs = thumbnail(a), thumbnail(b)
    n = min(len(ours), len(theirs))
    if not n:
        return None
    mse = sum((x - y) ** 2 for x, y in zip(ours[:n], theirs[:n])) / n
    if check:
        subprocess.run(['ffmpeg', '-nostdin', '-loglevel', 'error', '-y', '-i', a, '-i', b, '-filter_complex',
                        '[0:v]scale=512:-2[x];[1:v]scale=512:-2[y];[x][y]hstack', '-frames:v', '1', f'{check}.jpg'],
                       check=False)
    return (math.inf if mse == 0 else 10 * math.log10(255 ** 2 / mse)), min(std(ours[:n]), std(theirs[:n]))


def main(a: str, b: str, check: str = '', why: str = '') -> str:
    (one, ours, _, levels, wrong), (two, theirs, checks, others, wronger) = facts(a), facts(b)
    if one.get('status') != 'completed' or two.get('status') != 'completed':
        return f'FAIL runs ended {one.get("status")} and {two.get("status")}'
    if wrong or wronger:
        return f'FAIL a replay differed from eager: {wrong} and {wronger}'
    shared = sorted(ours.keys() & theirs.keys())
    if not shared:
        return f'FAIL no digest to compare: {sorted(ours)} against {sorted(theirs)}'
    different = [name for name in shared if ours[name] != theirs[name]]
    said = f'{len(shared)} digests, checks {checks}'
    if not different:
        return f'PASS byte-identical: {said}'
    whole = frozenset({'0'})
    sites = levels.keys() | others.keys()
    moved = sum(levels.get(site, whole) != others.get(site, whole) for site in sites)
    if not moved and not why:
        return f'FAIL {different} differ at the same split levels: {said}'
    reason = why or f'split levels moved at {moved} of {len(sites)} sites'
    try:
        judged = quality(output(one, a), output(two, b), check)
    except (OSError, subprocess.CalledProcessError) as error:
        return f'FAIL {different} differ ({reason}), and the outputs could not be judged: {error}'
    if judged is None:
        return f'FAIL {different} differ ({reason}), and there is no image or video to judge'
    psnr, flattest = judged
    verdict = 'PASS' if psnr >= FLOOR and flattest >= FLAT else 'FAIL'
    return (f'{verdict} by quality, {different} differ ({reason}): {psnr:.1f} dB at 32 px (floor {FLOOR}), '
            f'thumbnail std {flattest:.1f} (flat under {FLAT}): {said}')


if __name__ == '__main__':
    print(main(*sys.argv[1:5]))
