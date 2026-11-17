"""xs-204's CLI cells (llm-tools.sh), each from its record (`cozy --json --full run watch <n> --once`, <dir>/<cell>.json): where it ran,
its lane, total, step 1 and steady (each step track of its run and calls: the decode's tokens where it reports them), peak VRAM (TensorD's
NVML reads), kernels, and its answer whole. Then whether each answer describes what it was shown.

  python3 summary.py <dir> <cell>[=<pattern>] ...    (a pattern, case-insensitive, its answer must match)
"""
import os
import re
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', '..'))
from runs import Run  # noqa: E402  release-gate/runs.py


def tracks(run: Run) -> str:
    said: list[str] = []
    for m in run.measurements:
        for name, track in ((m.get('attribution') or {}).get('steps') or {}).items():
            series = [x[1] for x in track.get('series') or []]
            if not series:
                continue
            steady = f", steady {sum(series[1:]) / len(series[1:]):.1f} ms" if len(series) > 1 else ''
            said.append(f"{name}: {len(series)} steps, step 1 {series[0]:.0f} ms{steady}")
    return '; '.join(said) or 'no step tracks'


def main() -> int:
    d, cells = sys.argv[1], sys.argv[2:]
    fails: list[str] = []
    print('| cell | run | status | machine · GPU | model · lane | total | steps | peak VRAM (added) | attention · GEMM |')
    print('|---|---|---|---|---|---|---|---|---|')
    answers: list[tuple[str, int | None, str]] = []
    for cell in cells:
        name, _, pattern = cell.partition('=')
        run = Run.read(os.path.join(d, f'{name}.json'))
        if run is None:
            print(f'| {name} | - | no record | | | | | | |')
            fails.append(f'{name}: no record')
            continue
        rec = run.record
        models = ', '.join(f"{m.get('repository')} · {m.get('lane')}" for m in run.models) or '?'
        vram = run.vram
        said = run.result.get('text') if isinstance(run.result, dict) else None
        text = said if isinstance(said, str) else ''
        total = f'{run.execution_ms / 1000:.1f} s' if run.execution_ms else '?'
        peak = f"{vram['peak']} MiB ({vram['added']})" if vram else '?'
        print(f"| {name} | {run.number} | {run.status} | {rec.get('machine')} · GPU {','.join(map(str, run.gpus)) or '?'} | {models} | {total} |"
              f" {tracks(run)} | {peak} |"
              f" {', '.join(run.attention) or '-'} · {', '.join(k for _, k in run.gemm) or '-'} |")
        answers.append((name, run.number, text))
        if run.status != 'completed':
            fails.append(f'{name} (run {run.number}): {run.status} {run.reason}')
        elif not text.strip():
            fails.append(f'{name} (run {run.number}): no words')
        elif pattern and not re.search(pattern, text, re.I):
            fails.append(f'{name} (run {run.number}): its answer does not say /{pattern}/: {text[:200]}')
    print()
    for name, number, text in answers:
        print(f'**{name}** (run {number}):\n\n> ' + text.strip().replace('\n', '\n> ') + '\n')
    for fail in fails:
        print(f'FAIL {fail}')
    if not fails:
        print(f'PASS {len(cells)} answers, each describing what it was shown')
    return 1 if fails else 0


if __name__ == '__main__':
    sys.exit(main())
