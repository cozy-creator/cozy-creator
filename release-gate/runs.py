"""A run's record as `cozy run watch <n> --once --json --full` prints it (e2e/run.sh keeps it as <name>.json), read once into the
summary analyze.py and every check use. Its VRAM is the driver's (`vram`); varena's own figures are record.py's."""
import json
import os
from collections.abc import Iterator
from dataclasses import dataclass
from typing import NotRequired, Required, TypeAlias, TypedDict, cast

# Free JSON: a record's `result`, and whatever a walk over it meets.
JSON: TypeAlias = str | int | float | bool | None | list['JSON'] | dict[str, 'JSON']
# An observation's value or field (cozy_runtime.author._observations.Scalar).
Scalar: TypeAlias = str | int | float | bool | None


# The Runtime's host-stall codes (cozy_runtime/author/_stalls.py) that are a weight read: bytes from TensorFS into the
# executor's tensors, or the wait for a prefetched read to land.
WEIGHT_READS = ('cozy_runtime.internal.residency.Memory._read_into', 'cozy_runtime.internal.residency.Memory._settle_prefetch')
# A step may cost at most 2x its reference plus 5 s: step 1 against step 2 (known-good fresh H3 executors 0.9-1.8x; the
# 10-22 split regression 8-17x), every later step against the median of its phase (a length change, such as a video
# reference's rows leaving the sparse window, must not stall a step). The seconds absorb sub-second image steps.
STEP_RATIO, STEP_EXTRA_MS = 2, 5_000
# Steps 1..k of H3's series are their own phase: it attends densely for its first evaluations, a dense step costing
# about 2x a sparse one (minimax-h3 docs/attention-defaults.md: turbo 4 of 8, regular sampling the first 10). Against
# one median, a healthy turbo call with dense steps 2.07x its sparse ones failed (gate 40834197815).
def dense_steps(target: str, steps: int) -> int:
    return (4 if steps <= 8 else 10) if target == 'minimax-h3/generate' else 1
# The step tracks the step rule judges: a sampler's steps (`ctx.progress` stages). Not an autoregressive stage
# (HunyuanImage-3's `thinking`: token 1 is the prompt's prefill, gate 40907841888) and not per-item work (`decode_video`
# frames, `scan`, `assemble`, `calibrating`).
# 0.22's plain loops name a sampler's track after the function's stage: SDXL's is `generate` (runs 43985, 43986).
STEP_STAGES = ('denoise', 'generating image', 'refine', 'step', 'generate')


def slow_steps(target: str, s: list[float]) -> list[tuple[int, float, float, str]]:
    """Each step of the series `s` (ms) over STEP_RATIO x its reference + STEP_EXTRA_MS, as (step, ms, reference, what the
    reference is). `target` is canonical: pkg/fn."""
    if len(s) < 3:
        return []
    k = dense_steps(target, len(s))
    refs: list[tuple[float, str]] = [(s[1], 'step 2')]
    for phase, what in ((s[1:k], 'dense median'), (s[max(k, 1):], 'sparse median' if k > 1 else 'median')):
        if phase:
            refs += [(sorted(phase)[len(phase) // 2], what)] * len(phase)
    return [(i + 1, ms, ref, what) for i, (ms, (ref, what)) in enumerate(zip(s, refs)) if ms > STEP_RATIO * ref + STEP_EXTRA_MS]


# The record's fields this reader and its checks use (`cozy run show --json --full`). A JSON null reads as None.
class Reason(TypedDict, total=False):
    code: str
    message: str


class StepTrack(TypedDict, total=False):
    count: int
    total_ms: float
    first_ms: float
    series: list[list[float]]  # [unix ms at the step's end, ms]


class Stage(TypedDict, total=False):
    count: int
    total_ms: float
    first_ms: float
    started_unix_ms: float


class Attribution(TypedDict, total=False):
    steps: dict[str, StepTrack] | None
    stages: dict[str, Stage] | None


class DriverRow(TypedDict, total=False):
    """TensorD's NVML reads of one GPU over a call."""
    peak_bytes: int
    before_bytes: int
    sm_mhz_mean: float | None
    sm_mhz_limit: float | None
    throttled_share: float | None
    throttles: list[str] | None
    temperature_max_c: float | None
    power_max_w: float | None


class Fill(TypedDict, total=False):
    ns: int
    at_unix_ms: float


class Flow(TypedDict, total=False):
    """What moved one way in a request (`stats.moved[origin]`), or its bakes (`moved.bake`: device_ns)."""
    bytes: int
    ns: int
    device_ns: int


Moved: TypeAlias = dict[str, Flow]  # by origin (store, derived, kept, leased), and `bake`


class Outlived(TypedDict, total=False):
    bytes: int | None


class WeightResidency(TypedDict, total=False):
    fills: int | None
    evictions: int | None
    resident_bytes: int | None
    ready_bytes: int | None


class WorkingAllocator(TypedDict, total=False):
    weights_filled: int | None


class Refused(TypedDict, total=False):
    pure: bool | None
    level: int | str | None


class WorkingSplit(TypedDict, total=False):
    site: str | None
    refused: Refused | None


class EscapedLoop(TypedDict, total=False):
    """One loop's escaped record (tensorfs-py `_varena_escapes_take`): what its passes left live as plain working blocks."""
    head: str
    passes: int
    escaped_peak_bytes: int
    growth_bytes: int
    carried_bytes: int
    escaped_sites: list[tuple[str, int]]  # [birth site, bytes]


class VarenaStats(TypedDict, total=False):
    """Varena's Stats for one GPU (0.32): record.py's figures, the bench's, and the contract and escaped suites'."""
    trace: list[Fill] | None
    moved: Moved | None
    weight_residency: WeightResidency | None
    working_allocator: WorkingAllocator | None
    evicted: list[int] | None
    leftover: int | None
    outlived: list[Outlived | JSON] | None  # record.py skips other shapes
    host: int | None
    working_splits: list[WorkingSplit] | None
    swallowed_ooms: int | None
    finer_splits: int | None
    retained_bytes: int | None
    escaped: list[EscapedLoop] | None


class VarenaRow(TypedDict, total=False):
    stats: VarenaStats | None


class Copy(TypedDict, total=False):
    """0.30's copy record."""
    kind: str
    bytes: int | None
    seconds: float | None


class StreamRow(TypedDict, total=False):
    streamed: int | None


class Eviction(TypedDict, total=False):
    nbytes: int | None


class MemoryStats(TypedDict, total=False):
    varena: list[VarenaRow] | None  # 0.32
    copies: list[Copy] | None  # 0.30 from here
    rows: list[StreamRow | JSON] | None  # record.py skips other shapes
    paged_in: int | None
    evictions: list[Eviction | JSON] | None  # as rows
    leftover: int | None


class Memory(TypedDict, total=False):
    driver: list[DriverRow] | None
    stats: MemoryStats | None
    peak_reserved_bytes: int | None


class Loop(TypedDict, total=False):
    native_steps: int | None


class OnDemand(TypedDict, total=False):
    prepare_ms: float
    prepared_unix_ms: float


class Executor(TypedDict, total=False):
    """An executor's start, or a model's construction (no `started_unix_ms`)."""
    started_unix_ms: float
    legs_ms: dict[str, float] | None


class Execution(TypedDict, total=False):
    loops: list[Loop] | None
    safeguards: dict[str, list[str]] | None
    on_demand: dict[str, OnDemand] | None
    executor: Executor | None
    construction: Executor | None


class Graph(TypedDict, total=False):
    replays: int | None


class Residency(TypedDict, total=False):
    graphs: dict[str, Graph] | None


class Observation(TypedDict, total=False):
    name: str
    value: Scalar
    fields: dict[str, Scalar]


class Adapter(TypedDict, total=False):
    model: str | None
    strength: float | None
    components: list[str] | None


class ModelUse(TypedDict, total=False):
    slot: str
    repository: str
    lane: str
    adapters: list[Adapter] | None


class Measurements(TypedDict, total=False):
    attribution: Attribution | None
    memory: Memory | None
    execution: Execution | None
    residency: Residency | None
    observations: list[Observation] | None
    models: list[ModelUse] | None


class Call(TypedDict, total=False):
    function: str
    status: str
    called_at_ms: int | None
    memoized: bool | None
    measurements: Measurements | None


class OutputFile(TypedDict, total=False):
    path: str
    media_type: str
    sha256: str


class Memo(TypedDict, total=False):
    memoized: bool
    computation: str


class Request(TypedDict, total=False):
    seed: int | None
    input: dict[str, JSON]


class RunRecord(TypedDict, total=False):
    number: int
    status: str
    reason: Reason | str | None
    target: str
    machine: str | None
    gpu: str | None
    execution_ms: int | None
    created_at_ms: int | None
    started_at_ms: int | None
    settled_at_ms: int | None
    result: JSON
    output: list[OutputFile] | None
    measurements: Measurements | None
    calls: list[Call] | None
    memo: Memo | None
    request: Request | None


class Vram(TypedDict):
    peak: int
    added: int


@dataclass(frozen=True)
class Run:
    record: RunRecord
    number: int | None
    status: str  # queued, in_progress, completed, failed, canceled
    reason: str  # why it did not complete: `code: message`
    target: str
    execution_ms: int | None
    result: JSON
    outputs: tuple[str, ...]  # output[].path, sorted
    measurements: tuple[Measurements, ...]  # the run's, then each call's

    @staticmethod
    def of(rec: RunRecord) -> 'Run':
        why = rec.get('reason')
        return Run(record=rec, number=rec.get('number'), status=rec.get('status') or '',
                   reason=f"{why.get('code')}: {why.get('message')}" if isinstance(why, dict) else str(why or ''),
                   target=rec.get('target') or '', execution_ms=rec.get('execution_ms'), result=rec.get('result'),
                   outputs=tuple(sorted(o['path'] for o in rec.get('output') or [] if isinstance(o, dict) and o.get('path'))),
                   measurements=tuple([rec.get('measurements') or {}] + [c.get('measurements') or {} for c in rec.get('calls') or []]))

    @staticmethod
    def read(path: str) -> 'Run | None':
        """The record at `path`, None when there is none."""
        if not os.path.exists(path):
            return None
        with open(path) as f:
            return Run.of(cast(RunRecord, json.load(f)))  # the CLI's JSON, read as the fields above

    @property
    def observations(self) -> list[Observation]:
        return [o for m in self.measurements for o in m.get('observations') or []]

    @property
    def gemm(self) -> list[list[str]]:
        """[format, kernel] of every GEMM it selected."""
        # gemm.selected's format and kernel are names.
        return [list(x) for x in sorted({(cast(str, o['fields'].get('format', '')), cast(str, o['value'])) for o in self.observations if o.get('name') == 'gemm.selected'})]

    @property
    def attention(self) -> list[str]:
        return sorted({cast(str, o['value']) for o in self.observations if o.get('name') == 'attention.selected'})  # a kernel's name

    @property
    def graphs(self) -> str | None:
        """How its denoise loops ran: `native` when any step replayed a captured CUDA graph, `eager` when none did,
        None with no loop (a text model, a describer)."""
        loops = [l for m in self.measurements for l in (m.get('execution') or {}).get('loops') or []]
        if not loops:
            return None
        return 'native' if any((l.get('native_steps') or 0) > 0 for l in loops) else 'eager'

    @property
    def replays(self) -> int:
        """Steps a captured graph served: native loops', and each component's regions and decode steps."""
        return sum(sum(l.get('native_steps') or 0 for l in (m.get('execution') or {}).get('loops') or [])
                   + sum(g.get('replays') or 0 for g in ((m.get('residency') or {}).get('graphs') or {}).values())
                   for m in self.measurements)

    @property
    def safeguards(self) -> dict[str, list[str]]:
        """`execution.safeguards` over the run and its calls: {code: [why, ...]}."""
        found: dict[str, list[str]] = {}
        for m in self.measurements:
            for code, why in ((m.get('execution') or {}).get('safeguards') or {}).items():
                found.setdefault(code, []).extend(why)
        return found

    @property
    def capabilities_off(self) -> list[str]:
        """What the Runtime turned off on its executor (`capability.off`: CUDA graphs, Triton sets) and why."""
        return sorted({f"{o.get('value')} ({(o.get('fields') or {}).get('reason', '')})" for o in self.observations
                       if o.get('name') == 'capability.off'})

    @property
    def models(self) -> list[ModelUse]:
        """Each model it ran as the machine resolved it (`measurements.models`: slot, repository, lane, …)."""
        return [m for ms in self.measurements for m in ms.get('models') or []]

    def _step_track(self) -> tuple[Measurements | None, StepTrack | None]:
        """The dominant sampler step track (STEP_STAGES, largest total) of the run and its calls, with the measurement
        holding it."""
        best: tuple[Measurements | None, StepTrack | None] = (None, None)
        for m in self.measurements:
            for name, v in ((m.get('attribution') or {}).get('steps') or {}).items():
                if name not in STEP_STAGES:
                    continue
                if v.get('count') and (best[1] is None or v.get('total_ms', 0) > best[1].get('total_ms', 0)):
                    best = (m, v)
        return best

    @property
    def steps(self) -> list[float]:
        """The dominant step series of the run and its calls, in ms."""
        return [x[1] for x in (self._step_track()[1] or {}).get('series') or []]

    @property
    def step1_read_ms(self) -> float:
        """What step 1 spent getting its weights ready, once: its reads (the `host · <read>` stalls the Runtime's sampler
        logged at its end, where its stage's first entry ends, or its fills' own times, which see more than the sampler's
        last stall), a LoRA's construction inside the step and its bake (FLUX.2 Turbo on a 4090, 56 blocks: 15 s against
        1.6 s steps, on every Runtime it has run on). A fresh pod's first call does these inside step 1: the run's
        execution time and the tensorfs/varena suites judge them, not step compute."""
        m, track = self._step_track()
        series = (track or {}).get('series') or []
        if m is None or not series:
            return 0
        end = series[0][0]
        # The stall's entry is written right after the step's, in the same callback; the next step's ends later.
        window = min(50, (series[1][0] - end) / 2) if len(series) > 1 else 50
        ms = 0.0
        for name, st in ((m.get('attribution') or {}).get('stages') or {}).items():
            if name.startswith('host · ') and name[len('host · '):] in WEIGHT_READS and st.get('count'):
                if abs(st['started_unix_ms'] + st['first_ms'] - end) <= max(window, 2):
                    ms += st['first_ms']
        began = end - series[0][1]
        native = [row.get('stats') or {} for row in ((m.get('memory') or {}).get('stats') or {}).get('varena') or []]
        # A group's GPUs ready their weights at once: the slowest one's time, not their sum (cr-261).
        ms = max(ms, max((sum(fill.get('ns', 0) for fill in row.get('trace') or [] if began <= fill.get('at_unix_ms', 0) <= end) for row in native), default=0) / 1e6)
        built = [b for b in ((m.get('execution') or {}).get('on_demand') or {}).values() if began <= b.get('prepared_unix_ms', 0.0) <= end]
        ms += sum(b.get('prepare_ms', 0.0) for b in built)
        ms += max((((row.get('moved') or {}).get('bake') or {}).get('device_ns', 0) for row in native), default=0) / 1e6
        return min(ms, series[0][1])

    @property
    def to_step1_ms(self) -> int | None:
        """Its start to the end of its step 1, in ms: load and step 1 together. v2 readies weights on first use, inside
        step 1, where v1 readied them in a load before it, so this is the span that holds the same work on both."""
        m, track = self._step_track()
        series = (track or {}).get('series') or []
        if not series:
            return None
        i = next((n for n, x in enumerate(self.measurements) if x is m), 0)
        calls = self.record.get('calls') or []
        start = self.record.get('started_at_ms') if i == 0 else (calls[i - 1] or {}).get('called_at_ms')
        return round(series[0][0] - start) if start else None

    def stage_ms(self, name: str) -> float:
        return sum(((m.get('attribution') or {}).get('stages') or {}).get(name, {}).get('total_ms') or 0 for m in self.measurements)

    @property
    def executor_started_ms(self) -> float | None:
        """When the executor that ran it started: one value for every run it served."""
        return ((self.measurements[0].get('execution') or {}).get('executor') or {}).get('started_unix_ms')

    @property
    def gpus(self) -> list[int]:
        """The GPUs its measurements name."""
        def walk(o: object) -> Iterator[int]:
            if isinstance(o, dict):
                for k, v in o.items():
                    if k == 'gpu' and isinstance(v, int) and not isinstance(v, bool) and v >= 0:
                        yield v
                    yield from walk(v)
            elif isinstance(o, list):
                for v in o:
                    yield from walk(v)
        return sorted(set(walk(list(self.measurements))))

    @property
    def vram(self) -> Vram | None:
        """The driver's bytes in use on its fullest GPU (TensorD's NVML reads, `memory.driver`, over the run and each
        call): `peak`, all of them, and `added`, less those in use when it began, in MiB; None where none was read."""
        rows = [r for m in self.measurements for r in (m.get('memory') or {}).get('driver') or []]
        if not rows:
            return None
        return {'peak': round(max(r['peak_bytes'] for r in rows) / 2**20),
                'added': round(max(r['peak_bytes'] - r['before_bytes'] for r in rows) / 2**20)}

    def files(self, *ext: str) -> list[str]:
        return [p for p in self.outputs if p.endswith(ext)]

    @property
    def memoized(self) -> bool:
        """Answered, wholly or in part, from the machine's memo: a result it did not compute this time."""
        return bool((self.record.get('memo') or {}).get('memoized')) or any(c.get('memoized') for c in self.record.get('calls') or [])

    @property
    def seed(self) -> int | None:
        return (self.record.get('request') or {}).get('seed')


# A gate dir as analyze.py reads it and hands it to every check: check(gate) -> (fails, notes). A check's own `__main__`
# builds only the keys it reads, so each check takes the narrowest of these.
class Cell(TypedDict):
    """A completed cell's numbers (analyze.py's summary)."""
    blocking: dict[str, int]
    run: int | None
    target: str
    gemm: list[list[str]]
    attention: list[str]
    graphs: str | None
    execution_ms: int | None
    step1_ms: int | None
    step1_read_ms: int
    to_step1_ms: int | None
    vram_mb: int | None
    host: dict[str, int]
    steady_ms: int | None
    files: list[str]


class Good(TypedDict, total=False):
    """A cell's pass in the baseline file: Cell's numbers, its gate's label and when. An older pass may lack a later key."""
    run: int | None
    target: Required[str]
    gemm: Required[list[list[str]]]
    attention: Required[list[str]]
    graphs: str | None
    execution_ms: int | None
    step1_ms: int | None
    to_step1_ms: int | None
    steady_ms: int | None
    vram_mb: int | None
    blocking: dict[str, int]
    host: dict[str, int]
    label: str
    at: str


class CellHistory(TypedDict, total=False):
    good: list[Good]


class QualityRef(TypedDict, total=False):
    """A video cell's adherence on one prompt and seed (`key`), held for the next gate."""
    key: str
    adherence: float
    label: str


class QualityPass(TypedDict):
    sharpness: float
    adherence: float
    cells: dict[str, list[tuple[float, float]]]  # (adherence, sharpness) per image
    label: str


class VarenaPass(TypedDict):
    shares: dict[str, float]  # '<cell> <copy kind>': its share of the ceiling


class CaseGood(TypedDict):
    """A tensorfs suite case's known-good (suites/tensorfs/check.py): its time and bytes, and the gate that passed it."""
    seconds: float
    bytes: NotRequired[int]
    label: str
    at: str


class Suites(TypedDict, total=False):
    tensorfs: dict[str, list[CaseGood]]  # per case


class Baseline(TypedDict, total=False):
    """The SKU's baseline file (gate-<SKU>.json): each cell's passes and each check's history."""
    sku: str
    cells: dict[str, CellHistory]
    quality_ref: dict[str, QualityRef]
    quality: list[QualityPass]
    varena: list[VarenaPass]
    suites: Suites


class KnownFailing(TypedDict):
    """A known-failing.json row: a cell parked on a known open bug, not run until it is fixed."""
    sku: str  # or '*'
    cell: str
    issue: str
    what: str
    cells: NotRequired[list[str]]  # the cells it parks (varena's check reads these)


class GateDir(TypedDict):
    out: str  # the gate dir


class GateHistory(GateDir):
    baseline: Baseline


class GateRuns(GateDir):
    sku: str
    runs: dict[str, Run]  # the completed cells


class Gate(GateHistory, GateRuns):
    summary: dict[str, Cell]
