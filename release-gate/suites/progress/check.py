"""The progress suite's verdict (cl-921), one of analyze.py's CHECKS. Each cell's event stream (<cell>.ndjson, what `cozy run
--json` printed, the same events the page draws) is read as the phases the run went through, and every one must say what it
is doing, where, how far and how long the whole run has left:

- **where:** from the moment it runs, the run names its machine; by its end, the GPU.
- **in order:** placed, rented, queued, made ready (inputs, install, weights), started, then its work (loads, compiles, stages
  and steps, decode, encode, save), then its result coming here; a call inside it (`Segment k of n / …`) in its own order. A cold
  cell (`-cold`) installs, and its weights say their bytes coming (downloaded first, or streamed as it loads them).
- **numbers:** a download, a load and the result count bytes; a stage that counts says what it counts.
- **words:** every stage it names is one the page knows (`web/app/src/panels/runs/stages.json`); a count the page calls
  steps is a denoising loop's, never tokens or a cap (Paul, 11-12: "Denoise steps: 65 / 2176" was a rewriter's tokens).
- **a time left:** a warm cell's throughout; a cold cell's while bytes move at a known rate, and while a stage steps from its
  second step (what follows a first run's last stepping stage isn't known yet); a result over RESULT_TIMED as it comes.
- **never silent:** no stretch of over SILENT_MS where nothing moved, and never `stalled`.
- **no false alarms:** a run that completed warns of nothing the Runtime caught and recovered from (`HANDLED`).

A `-memo` cell is a request asked again and answered from its first: only where and warnings hold.

    check.py <gate dir> <SKU>     the same verdict alone"""
import json
import os
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from collections.abc import Callable
from typing import cast

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, '..', '..'))
from runs import GateRuns  # noqa: E402
sys.path.insert(0, os.path.join(HERE, '..', '..', '..', 'e2e', 'cli'))
import stages as stages_py  # noqa: E402  (e2e/cli/stages.py: the one reader of the page's map)

# The page's map of stage words (cl-921: one map, read by the page and here).
STAGES = os.path.join(HERE, '..', '..', '..', 'web', 'app', 'src', 'panels', 'runs', 'stages.json')
# Nothing moved for longer than this while it ran: the box reads as stuck (46780 sat 109 s in `encoding prompt`).
SILENT_MS = 60_000
# A result this big takes long enough coming here to need its time left.
RESULT_TIMED = 50 << 20
# A denoising loop's steps: more is a cap or tokens read as steps.
MOST_STEPS = 200
# The order of phases (stages.json's `phases`, grouped). A machine gets ready all at once (cl-905: install, download and inputs
# together), and work phases interleave (a load or a compile happens inside a stage), so each group is one rank.
ORDER = ['placing', 'renting', 'queued', 'setup', 'starting', 'work', 'result']
SETUP = {'uploading', 'preparing', 'installing', 'downloading'}
STARTING = {'importing', 'starting'}
WORK = {'loading', 'compiling', 'working', 'saving'}
# The phases whose count is bytes moving at a rate: their time left is known from it.
BYTES = {'downloading', 'loading', 'result', 'uploading'}
# What the Runtime caught and recovered from (graphs.SAFEGUARDS, the LM's eager decode): never a warning on a run that
# completed (Paul, 11-12, on 46876's refused captures: "just got a bunch of errors"); the record counts them.
HANDLED = re.compile(r"CUDA graph( was refused at capture| was not used| differed from eager|'s pool stays mapped)|kept CUDA graph had no room", re.I)

Event = dict[str, object]
Entry = stages_py.Entry


@dataclass
class Stages:
    """The page's map (`web/app/src/panels/runs/stages.json`) as the e2e cells read it (`e2e/cli/stages.py`): each part of a stage
    path, matched in order, its words, its phase and what its count counts; a scope part (`Segment k of n`) has neither."""
    entries: list[tuple[Entry, list[str], re.Pattern[str]]]

    @classmethod
    def read(cls, path: str) -> 'Stages':
        return cls(stages_py.compiled(Path(path)))

    def of(self, part: str) -> Entry | None:
        return stages_py.said(self.entries, part)


@dataclass
class Seen:
    """One observation of the run, in stream order."""
    kind: str  # a phase of stages.json, or of Calcifer's waits
    stage: str
    at_ms: int | None
    progress: dict[str, object] = field(default_factory=dict)
    known: bool = True
    # A `run.log` stage line: its words are checked, but it arrives in batches behind the progress it repeats, so it holds no place.
    logged: bool = False


def parts(stage: str) -> list[str]:
    return [part.strip() for part in stage.split(' / ') if part.strip()]


def own(stage: str) -> str:
    return (parts(stage) or [''])[-1]


def phase_of(stage: str, stages: Stages) -> tuple[str, bool]:
    """A stage path's phase, its last part's (a scope alone is work), and whether the map knows every part."""
    entries = [stages.of(part) for part in parts(stage)]
    last = entries[-1] if entries else None
    phase = 'scope' if last and last.get('scope') else (last.get('phase') or 'working') if last else 'working'
    return phase, all(e is not None for e in entries)


def read(path: str, stages: Stages) -> tuple[list[Event], list[Seen]]:
    events: list[Event] = []
    for line in open(path):
        if line.startswith('{'):
            try:
                events.append(json.loads(line))
            except ValueError:
                pass
    seen: list[Seen] = []
    for e in events:
        kind = e.get('type')
        if kind == 'run.progress':
            p = cast(dict[str, object], e.get('progress') or {})
            stage = str(p.get('stage') or '')
            phase, known = phase_of(stage, stages)
            seen.append(Seen(phase, stage, cast(int | None, p.get('at_ms')), p, known))
        elif kind == 'run.arriving':
            p = cast(dict[str, object], e.get('progress') or {})
            seen.append(Seen('result', 'downloading the result', cast(int | None, p.get('at_ms')), p))
        elif kind == 'run.waiting' and e.get('reason'):
            reason = str(e['reason'])
            stage = re.sub(r'^(queued|preparing|running): ', '', reason)
            what = 'renting' if stage.startswith('renting ') else 'uploading' if re.match(r'(uploading|waiting for .* to upload)', stage) \
                else 'downloading' if 'waiting for required model inputs' in stage else 'queued'
            seen.append(Seen(what, stage, None))
        elif kind == 'run.log' and e.get('level') == 'stage':
            stage = str(e.get('text') or '')
            phase, known = phase_of(stage, stages)
            seen.append(Seen(phase, stage, cast(int | None, e.get('at_ms')), known=known, logged=True))
        elif isinstance(e.get('run'), dict):
            run = cast(dict[str, object], e['run'])
            if run.get('queue_position') is not None:
                seen.append(Seen('queued', f"number {run['queue_position']} in line", None))
            elif not run.get('machine') and run.get('status') == 'queued':
                seen.append(Seen('placing', 'choosing a machine', None))
    return events, seen


def records(events: list[Event]) -> list[dict[str, object]]:
    return [cast(dict[str, object], e['run']) for e in events if isinstance(e.get('run'), dict)]


def out_of_order(kinds: list[str], rank: 'Callable[[str], int]') -> str | None:
    """The first phase that goes back: a download during work is an on-demand model's and may come late; loads and compiles happen
    whenever weights or kernels are first wanted, before an executor starts too (`warming`), so they hold no place."""
    started = next((i for i, k in enumerate(kinds) if k in WORK), len(kinds))
    for i, kind in enumerate(kinds):
        if kind in ('loading', 'compiling'):
            continue
        before = [rank(k) for k in kinds[:i] if k not in ('loading', 'compiling')]
        if before and rank(kind) < max(before) and not (kind == 'downloading' and i > started) and kind not in WORK:
            return f'{kind} after {kinds[i - 1]}'
    return None


def judge(name: str, events: list[Event], seen: list[Seen], stages: Stages) -> list[str]:
    fails: list[str] = []
    cold = name.endswith('-cold')
    runs = records(events)
    # Where: its machine from the moment it runs; its GPU by its end.
    going = [r for r in runs if r.get('status') == 'in_progress']
    if going and not all(r.get('machine') for r in going):
        fails.append(f'{name}: running with no machine named')
    if runs and runs[-1].get('status') == 'completed' and not runs[-1].get('gpu'):
        fails.append(f'{name}: completed with no GPU named')
    if runs and runs[-1].get('status') == 'completed':
        said = [str(e.get('text')) for e in events if e.get('type') == 'run.log' and e.get('level') in ('warning', 'error')]
        said += [str(w.get('message')) for w in cast(list[dict[str, object]], runs[-1].get('warnings') or [])]
        said += [str(a.get('reason')) for a in cast(list[dict[str, object]], runs[-1].get('adjustments') or []) if a.get('kind') == 'warn']
        for text in sorted({t for t in said if HANDLED.search(t)}):
            fails.append(f'{name}: completed, yet warns of what the Runtime recovered from: {text[:160]!r}')
    if any(r.get('stalled') for r in runs):
        fails.append(f'{name}: stalled ({next(r["stalled"] for r in runs if r.get("stalled"))})')
    # In order.
    rank = lambda kind: ORDER.index('work' if kind in WORK else 'setup' if kind in SETUP else 'starting' if kind in STARTING else kind)
    placed = [s for s in seen if not s.logged and s.kind != 'scope']
    kinds = [s.kind for s in placed]
    # A call inside the run (`Segment 1 of 2 / …`, an orchestrator's child) goes through its own phases: order holds within each.
    for scope in sorted({' / '.join(parts(s.stage)[:-1]) for s in placed}):
        own_kinds = [s.kind for s in placed if ' / '.join(parts(s.stage)[:-1]) == scope]
        bad = out_of_order(own_kinds, rank)
        if bad:
            fails.append(f'{name}: {bad} out of order{f" in {scope!r}" if scope else ""}')
            break
    if cold:
        if 'installing' not in kinds:
            fails.append(f'{name}: a cold run with no installing phase')
        # Its weights come down before it runs (`downloading`) or as it reads them (a load that counts their bytes).
        moved = [s for s in seen if s.kind in ('downloading', 'loading') and s.progress.get('bytes_total')]
        if not moved:
            fails.append(f'{name}: a cold run whose weights never said their bytes coming (no download or load with bytes)')
    if not name.endswith('-memo') and not any(k in WORK for k in kinds):
        fails.append(f'{name}: no work phase seen')
    # Numbers and words: every part of every stage is one the page knows; a step is a denoising loop's.
    unknown = sorted({s.stage for s in seen if not s.known})
    for stage in unknown:
        fails.append(f'{name}: the page has no words for stage {stage!r}')
    for s in seen:
        p = s.progress
        if not p:
            continue
        entry = stages.of(own(s.stage))
        counts_bytes = s.kind == 'result' or bool(entry and entry.get('counts') == 'bytes')
        if counts_bytes and not p.get('bytes_total') and not p.get('bytes_done'):
            fails.append(f'{name}: {s.kind} {s.stage!r} counts bytes and said none')
        total, unit = int(cast(int, p.get('total') or 0)), str(p.get('unit') or '')
        if entry and entry.get('counts') == 'step' and total:
            if entry.get('phase') != 'working':
                fails.append(f'{name}: {own(s.stage)!r} counts steps outside work')
            elif unit:
                fails.append(f'{name}: {own(s.stage)!r} counts {unit} where the page says steps')
            elif total > MOST_STEPS:
                fails.append(f'{name}: {own(s.stage)!r} counts {p.get("completed")} of {total} as steps: no denoising loop has {total}')
    # A time left: a warm run's from its start; a cold run's while bytes move at a known rate, and while a stage steps from its
    # second step (what follows a first run's last stepping stage isn't known yet).
    for s in seen:
        p = s.progress
        if not p or name.endswith('-memo'):
            continue
        # The result coming here is said in time only when it takes a while: a picture lands in under a second.
        if s.kind == 'result' and int(cast(int, p.get('bytes_total') or 0)) < RESULT_TIMED:
            continue
        stepping = int(cast(int, p.get('completed') or 0)) >= 2 and int(cast(int, p.get('total') or 0)) > 0 and not p.get('unit')
        due = (not cold) or stepping or (s.kind in BYTES and p.get('bytes_per_s'))
        if due and p.get('eta_s') is None:
            fails.append(f'{name}: no time left at {s.kind} {s.stage!r} ({p.get("eta_basis") or "no basis"})')
            break
    # Never silent.
    moves = [s.at_ms for s in seen if s.at_ms]
    gaps = [(b - a, i) for i, (a, b) in enumerate(zip(moves, moves[1:]))]
    if gaps:
        gap, i = max(gaps)
        if gap > SILENT_MS:
            during = next((s.stage for s in seen if s.at_ms == moves[i]), '?')
            fails.append(f'{name}: nothing moved for {gap / 1000:.0f} s during {during!r}')
    return fails


def check(gate: GateRuns) -> tuple[list[str], list[str]]:
    out = gate['out']
    stages = Stages.read(STAGES)
    fails: list[str] = []
    notes: list[str] = []
    for line in open(os.path.join(out, 'cells.tsv')):
        name = line.split('\t')[0]
        # `-memo`: a request run again as asked, answered from its first (no work of its own); its checks are where and warnings.
        if not name.endswith(('-cold', '-warm', '-memo')):
            continue
        path = os.path.join(out, name + '.ndjson')
        if not os.path.exists(path):
            fails.append(f'{name}: no event stream')
            continue
        events, seen = read(path, stages)
        found = judge(name, events, seen, stages)
        fails += found
        order: list[str] = []
        for s in seen:
            if not order or order[-1] != s.kind:
                order.append(s.kind)
        notes.append(f'{name}: {" > ".join(order)}{"" if found else " (PASS)"}')
    return fails, notes


if __name__ == '__main__':
    out, sku = sys.argv[1].rstrip('/'), sys.argv[2]
    fails, notes = check(cast(GateRuns, {'out': out, 'sku': sku, 'runs': {}}))
    for n in notes:
        print('note', n)
    for f in fails:
        print('FAIL', f)
    print('PROGRESS', 'FAIL' if fails else 'PASS')
    sys.exit(1 if fails else 0)
