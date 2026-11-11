"""Every cell served the attention table's choice (xs-171, measurement-design §4.4). For each component of an unpinned run,
each band's kernel must be the head of the table's cell for the run's hardware class and the site's layout, level and head
dim, or a package producer standing in before that head (SDPA beside it is a call the head refused). The table is the candidate's (`attention-table.json` in the gate
directory, gate.sh); the records name `attention.class` and `attention.sites` (Runtime xs-171 R2, R4). A run without them,
or a gate without a table, is noted, never failed. A fallback is analyze.py's FAIL already.
    check(gate) -> (fails, notes)"""
import json
import os
from collections.abc import Iterator, Mapping
from typing import TypedDict


class Cell(TypedDict):
    head_dim: int | None
    order: list[list[str]]


class Site(TypedDict, total=False):
    """A component's sites at one level (runtime-design §8, R4): their facts, and the kernel each band served. A band
    `+`-joins kernels when refusals split it; a one-rung site has the band ""; head_dim 0 is a module that doesn't say."""
    layout: str
    level: int
    head_dim: int
    sites: int
    bands: dict[str, str]
    producers: dict[str, str]


class Attention(TypedDict, total=False):
    requested: str
    observed: str
    sites: dict[str, list[Site]]


def band(edges: list[int], b: int) -> str:
    """runtime-design §8's band label: `<2048`, `2048-12287`, `>=65536`; empty for one band."""
    if not edges:
        return ''
    if b == 0:
        return f'<{edges[0]}'
    return f'>={edges[-1]}' if b == len(edges) else f'{edges[b - 1]}-{edges[b] - 1}'


def heads(table: dict[str, object]) -> dict[tuple[str, str, int, int | None, str], str]:
    edges = table['tokens']
    assert isinstance(edges, list)
    found = {}
    for cell in table['cells']:  # type: ignore[attr-defined]
        for b, ranking in enumerate(cell['order']):
            found[(cell['class'], cell['layout'], cell['level'], cell['head_dim'], band(edges, b))] = ranking[0]
    return found


def attentions(record: object) -> Iterator[Attention]:
    """Every process's `attention` in a run's record, its calls' included."""
    if isinstance(record, dict):
        found = record.get('attention')
        if isinstance(found, dict) and 'observed' in found:
            yield found  # type: ignore[misc]
        for value in record.values():
            yield from attentions(value)
    elif isinstance(record, list):
        for value in record:
            yield from attentions(value)


def check(gate: Mapping[str, object]) -> tuple[list[str], list[str]]:
    fails: list[str] = []
    notes: list[str] = []
    out, runs = str(gate['out']), gate['runs']
    assert isinstance(runs, dict)
    path = os.path.join(out, 'attention-table.json')
    if not os.path.exists(path):
        return fails, ['served kernels: no attention-table.json for this candidate, not judged']
    head = heads(json.load(open(path)))
    judged = unjudged = 0
    for name in sorted(runs):
        record = json.load(open(os.path.join(out, f'{name}.json')))
        for att in attentions(record):
            if att.get('requested') or not att.get('observed'):
                continue
            klass, sites = att.get('class'), att.get('sites')
            if not isinstance(klass, str) or not sites:
                unjudged += 1
                continue
            judged += 1
            for component, rows in sorted(sites.items()):
                for site in rows:
                    for label, served in sorted((site.get('bands') or {}).items()):
                        level, layout, dim = site.get('level', 16), site.get('layout', 'dense'), site.get('head_dim') or None
                        bands = [label] if label else sorted({k[4] for k in head if k[:3] == (klass, layout, level)})
                        for b in bands:
                            want = head.get((klass, layout, level, dim, b)) or head.get((klass, layout, level, None, b))
                            # A call the head refuses runs on SDPA: `head+sdpa` is the head serving.
                            kernels = [k for k in served.split('+') if k != 'sdpa' or served == 'sdpa']
                            ok = want is not None and all(k == want or (site.get('producers') or {}).get(k) == want for k in kernels)
                            if want is None:
                                fails.append(f'{name}: {component} served {served} in band {b or "all"}, and the table has no {klass} {layout} {level} cell')
                            elif not ok:
                                fails.append(f'{name}: {component} served {served} in band {b or "all"}; the table\'s head for {klass} {layout} {level} is {want}')
    if unjudged:
        notes.append(f'served kernels: {unjudged} records name no attention class and sites (a Runtime before xs-171 R4), not judged')
    if judged:
        notes.append(f'served kernels: {judged} records held to the table')
    return fails, notes
