"""A run's record as `cozy run show <n> --once --json` prints it (e2e/run.sh keeps it as <name>.json), read once into the
summary analyze.py and every check use. Its VRAM is the driver's (`vram`); varena's own figures are record.py's."""
import json
import os
from dataclasses import dataclass


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
def dense_steps(target, steps):
    return (4 if steps <= 8 else 10) if target == 'minimax-h3/generate' else 1
# The step tracks the step rule judges: a sampler's steps (`tel.step_callback` stages). Not an autoregressive stage
# (HunyuanImage-3's `thinking`: token 1 is the prompt's prefill, gate 40907841888) and not per-item work (`decode_video`
# frames, `scan`, `assemble`, `calibrating`).
STEP_STAGES = ('denoise', 'generating image', 'refine', 'step')


def slow_steps(target, s):
    """Each step of the series `s` (ms) over STEP_RATIO x its reference + STEP_EXTRA_MS, as (step, ms, reference, what the
    reference is). `target` is canonical: pkg/fn."""
    if len(s) < 3:
        return []
    k = dense_steps(target, len(s))
    refs = [(s[1], 'step 2')]
    for phase, what in ((s[1:k], 'dense median'), (s[max(k, 1):], 'sparse median' if k > 1 else 'median')):
        if phase:
            refs += [(sorted(phase)[len(phase) // 2], what)] * len(phase)
    return [(i + 1, ms, ref, what) for i, (ms, (ref, what)) in enumerate(zip(s, refs)) if ms > STEP_RATIO * ref + STEP_EXTRA_MS]


@dataclass(frozen=True)
class Run:
    record: dict
    number: int | None
    status: str  # queued, in_progress, completed, failed, canceled
    reason: str  # why it did not complete: `code: message`
    target: str
    execution_ms: int | None
    result: object
    outputs: tuple  # output[].path, sorted
    measurements: tuple  # the run's, then each call's

    @staticmethod
    def of(rec):
        why = rec.get('reason')
        return Run(record=rec, number=rec.get('number'), status=rec.get('status') or '',
                   reason=f"{why.get('code')}: {why.get('message')}" if isinstance(why, dict) else str(why or ''),
                   target=rec.get('target') or '', execution_ms=rec.get('execution_ms'), result=rec.get('result'),
                   outputs=tuple(sorted(o['path'] for o in rec.get('output') or [] if isinstance(o, dict) and o.get('path'))),
                   measurements=tuple([rec.get('measurements') or {}] + [c.get('measurements') or {} for c in rec.get('calls') or []]))

    @staticmethod
    def read(path):
        """The record at `path`, None when there is none."""
        return Run.of(json.load(open(path))) if os.path.exists(path) else None

    @property
    def observations(self):
        return [o for m in self.measurements for o in m.get('observations') or []]

    @property
    def gemm(self):
        """[format, kernel] of every GEMM it selected."""
        return [list(x) for x in sorted({(o['fields'].get('format', ''), o['value']) for o in self.observations if o.get('name') == 'gemm.selected'})]

    @property
    def attention(self):
        return sorted({o['value'] for o in self.observations if o.get('name') == 'attention.selected'})

    @property
    def models(self):
        """Each model it ran as the machine resolved it (`measurements.models`: slot, repository, lane, …)."""
        return [m for ms in self.measurements for m in ms.get('models') or []]

    def _step_track(self):
        """The dominant sampler step track (STEP_STAGES, largest total) of the run and its calls, with the measurement
        holding it."""
        best = (None, None)
        for m in self.measurements:
            for name, v in ((m.get('attribution') or {}).get('steps') or {}).items():
                if name not in STEP_STAGES:
                    continue
                if v.get('count') and (best[1] is None or v.get('total_ms', 0) > best[1].get('total_ms', 0)):
                    best = (m, v)
        return best

    @property
    def steps(self):
        """The dominant step series of the run and its calls, in ms."""
        return [x[1] for x in (self._step_track()[1] or {}).get('series') or []]

    @property
    def step1_read_ms(self):
        """What step 1 waited on weight reads: the `host · <read>` stalls the Runtime's sampler logged at step 1's
        end (its stage's first entry ends there). A fresh pod's first call reads its weights from a cold disk inside
        step 1; that is I/O, which the run's execution time and the tensorfs/varena suites judge, not step compute."""
        m, track = self._step_track()
        series = (track or {}).get('series') or []
        if not series:
            return 0
        end = series[0][0]
        # The stall's entry is written right after the step's, in the same callback; the next step's ends later.
        window = min(50, (series[1][0] - end) / 2) if len(series) > 1 else 50
        ms = 0.0
        for name, st in ((m.get('attribution') or {}).get('stages') or {}).items():
            if name.startswith('host · ') and name[len('host · '):] in WEIGHT_READS and st.get('count'):
                if abs(st['started_unix_ms'] + st['first_ms'] - end) <= max(window, 2):
                    ms += st['first_ms']
        return min(ms, series[0][1])

    def stage_ms(self, name):
        return sum(((m.get('attribution') or {}).get('stages') or {}).get(name, {}).get('total_ms') or 0 for m in self.measurements)

    @property
    def executor_started_ms(self):
        """When the executor that ran it started: one value for every run it served."""
        return ((self.measurements[0].get('execution') or {}).get('executor') or {}).get('started_unix_ms')

    @property
    def gpus(self):
        """The GPUs its measurements name."""
        def walk(o):
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
    def vram(self):
        """The driver's bytes in use on its fullest GPU (TensorD's NVML reads, `memory.driver`, over the run and each
        call): `peak`, all of them, and `added`, less those in use when it began, in MiB; None where none was read."""
        rows = [r for m in self.measurements for r in (m.get('memory') or {}).get('driver') or []]
        if not rows:
            return None
        mib = lambda b: round(b / 2**20)
        return {'peak': mib(max(r['peak_bytes'] for r in rows)),
                'added': mib(max(r['peak_bytes'] - r['before_bytes'] for r in rows))}

    def files(self, *ext):
        return [p for p in self.outputs if p.endswith(ext)]
