#!/usr/bin/env python3
"""The three bar choices Paul signs between, each with the table it gives and what it costs end to end (xs-171 F).

    options.py OUT --runtime CHECKOUT --measurements FILE GATE_DIR...

CHECKOUT is a cozy-runtime checkout whose `scripts/attention_table.py` generates; its shipped table is today's choice.
FILE is collect.py's measurements of the calibration round; the gate directories hold calibrate.sh's arms. OUT gets
tables/<option>.json and .txt (each generator report: every kernel the bars left out and why) and options.md:
- (a) a level-8 bar that admits today's 8-bit kernels (their worst captured error, rounded up to 0.005);
- (b) the provisional bar, with the layers where today's 8-bit kernels miss it held to 16 bits (their captured sites
  taken out of the judging, as a per-layer 16-bit exception would);
- (c) the provisional bar as it is.
Per class: each option's 8-bit cells (head per token band), and each model's arm times (16-bit, today's choice, each
8-bit kernel) with the arm that option comes closest to. Nothing here is committed to the Runtime: Paul signs first."""

import json
import math
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import cast

MODEL_OF = {'minimax-h3': 'H3 15 s', 'qwen-image-2': 'Qwen-Image 2.1 4 MP', 'moss-image': 'MOSS 4 MP', 'flux.3-klein': 'Klein'}


def captured(measurements: dict[str, object]) -> dict[str, dict[str, list[dict[str, object]]]]:
    """class -> kernel -> its captured-site errors."""
    found: dict[str, dict[str, list[dict[str, object]]]] = {}
    for klass, record in cast(dict[str, dict[str, object]], measurements['classes']).items():
        for kernel, q in cast(dict[str, dict[str, object]], record.get('quality') or {}).items():
            found.setdefault(klass, {})[kernel] = [e for e in cast(list[dict[str, object]], q.get('errors') or []) if e.get('kind') == 'captured']
    return found


def today(table: dict[str, object]) -> dict[str, set[str]]:
    """class -> the kernels today's table lists in its 8-bit cells."""
    found: dict[str, set[str]] = {}
    for cell in cast(list[dict[str, object]], table['cells']):
        if cell['level'] == 8:
            found.setdefault(str(cell['class']), set()).update(r[0] for r in cast(list[list[str]], cell['order']) if r)
    return found


def layer(site: str) -> str:
    """`klein/14/transformer__single_transformer_blocks.0.attn.0049.safetensors` -> `klein/transformer__single_..._0.attn`."""
    model, _, rest = site.split('/', 2)
    return f"{model}/{rest.rsplit('.', 2)[0]}"


def generate(runtime: Path, measurements: Path, rules: Path, table: Path, report: Path, partial: bool) -> None:
    command = ['uv', 'run', '-q', '--frozen', '--no-dev', 'python', 'scripts/attention_table.py', 'generate',
               '--measurements', str(measurements), '--rules', str(rules), '--table', str(table), '--report', str(report)]
    subprocess.run(command + (['--partial'] if partial else []), cwd=runtime, check=True)


def band(edges: list[int], b: int) -> str:
    """runtime-design §8's band label (served.py's)."""
    if not edges:
        return 'all'
    if b == 0:
        return f'<{edges[0]}'
    return f'>={edges[-1]}' if b == len(edges) else f'{edges[b - 1]}-{edges[b] - 1}'


def heads(table: Path) -> dict[tuple[str, str, int], list[str]]:
    """(class, layout, level) -> `band: head` for each token band (the catch-all head dim's cell)."""
    data = json.loads(table.read_text())
    edges = cast(list[int], data['tokens'])
    found: dict[tuple[str, str, int], list[str]] = {}
    for cell in cast(list[dict[str, object]], data['cells']):
        if cell.get('head_dim') is None:
            found[(str(cell['class']), str(cell['layout']), int(cast(int, cell['level'])))] = [
                f"{band(edges, b)}: {r[0] if r else '-'}" for b, r in enumerate(cast(list[list[str]], cell['order']))]
    return found


def dense_steps(record: dict[str, object], n: int = 4) -> float:
    """The mean of a run's first N denoise steps, s (H3 Turbo's dense steps)."""
    for call in cast(list[dict[str, object]], record.get('calls') or []):
        series = cast(dict[str, object], cast(dict[str, object], cast(dict[str, object], call.get('measurements') or {}).get('attribution') or {}).get('steps') or {}).get('denoise')
        steps = cast(list[list[float]], cast(dict[str, object], series or {}).get('series') or [])
        if len(steps) > n:
            return sum(s[1] for s in steps[:n]) / n / 1000
    return 0.0


def arms(gates: list[Path]) -> tuple[dict[str, dict[str, dict[str, float]]], dict[str, dict[str, dict[str, float]]]]:
    """class -> model -> arm -> its mean execution seconds over the arm requests (calibrate.sh's runs); and the same for
    the mean of the first four denoise steps (H3's dense steps)."""
    total: dict[str, dict[str, dict[str, list[float]]]] = {}
    dense: dict[str, dict[str, dict[str, list[float]]]] = {}
    for gate in gates:
        rows = gate / 'signoff' / 'signoff.jsonl'
        for line in rows.read_text().splitlines() if rows.exists() else []:
            r = json.loads(line)
            try:
                record = json.loads((gate / f"calibrate-{r['cell']}-{r['arm']}.json").read_text())
            except (OSError, ValueError):
                continue
            if record.get('execution_ms'):
                total.setdefault(r['class'], {}).setdefault(r['model'], {}).setdefault(r['arm'], []).append(record['execution_ms'] / 1000)
                dense.setdefault(r['class'], {}).setdefault(r['model'], {}).setdefault(r['arm'], []).append(dense_steps(record))

    def mean(d: dict[str, dict[str, dict[str, list[float]]]]) -> dict[str, dict[str, dict[str, float]]]:
        return {k: {m: {a: round(sum(v) / len(v), 1) for a, v in by.items()} for m, by in models.items()} for k, models in d.items()}
    return mean(total), mean(dense)


def main(argv: list[str]) -> int:
    if len(argv) < 6 or argv[2] != '--runtime' or argv[4] != '--measurements':
        print(__doc__, file=sys.stderr)
        return 2
    out, runtime, source, gates = Path(argv[1]), Path(argv[3]).resolve(), Path(argv[5]).resolve(), [Path(g) for g in argv[6:]]
    (out / 'tables').mkdir(parents=True, exist_ok=True)
    measurements = json.loads(source.read_text())
    rules_text = (runtime / 'measurements' / 'rules.toml').read_text()
    bar = float(cast(re.Match[str], re.search(r'level = \{[^}]*8 = ([0-9.]+)', rules_text)).group(1))
    current = today(json.loads((runtime / 'src/cozy_runtime/internal/attention_table.json').read_text()))
    errors = captured(measurements)
    # Today's dense 8-bit kernels set (a); a sparse kernel is judged against its own route by the generator, not here.
    sparse = {str(k) for k, q in ((k, q) for r in cast(dict[str, dict[str, object]], measurements['classes']).values()
                                  for k, q in cast(dict[str, dict[str, object]], r.get('quality') or {}).items()) if q.get('reference')}
    current = {k: {kernel for kernel in ks if kernel not in sparse} for k, ks in current.items()}
    worst_today = max((cast(float, e['worst']) for k, ks in current.items() for kernel in ks for e in errors.get(k, {}).get(kernel, [])), default=bar)
    bar_a = round(math.ceil(worst_today / 0.005) * 0.005, 3)
    flagged = sorted({layer(str(e['site'])) for k, ks in current.items() for kernel in ks for e in errors.get(k, {}).get(kernel, [])
                      if cast(float, e['worst']) > bar})
    partial = len(cast(dict[str, object], measurements['classes'])) < 6
    with tempfile.TemporaryDirectory(prefix='options-') as tmp:
        work = Path(tmp)
        (work / 'a.toml').write_text(re.sub(r'(level = \{[^}]*8 = )[0-9.]+', rf'\g<1>{bar_a}', rules_text))
        (work / 'base.toml').write_text(rules_text)
        held = json.loads(json.dumps(measurements))
        for record in cast(dict[str, dict[str, object]], held['classes']).values():
            for q in cast(dict[str, dict[str, object]], record.get('quality') or {}).values():
                q['errors'] = [e for e in cast(list[dict[str, object]], q.get('errors') or [])
                               if not (e.get('kind') == 'captured' and layer(str(e['site'])) in flagged)]
        (work / 'held.json').write_text(json.dumps(held))
        for option, m, rules in (('a', source, work / 'a.toml'), ('b', work / 'held.json', work / 'base.toml'), ('c', source, work / 'base.toml')):
            generate(runtime, m, rules, out / 'tables' / f'{option}.json', out / 'tables' / f'{option}.txt', partial)
    tables = {o: heads(out / 'tables' / f'{o}.json') for o in 'abc'}
    times, dense = arms(gates)
    lines = ['# The bars Paul signs: three choices and what each costs', '',
             f'- **(a)** level-8 bar {bar_a}: admits today\'s 8-bit kernels (their worst captured error {worst_today:.3f}).',
             f'- **(b)** level-8 bar {bar}, with these layers held to 16 bits (today\'s 8-bit kernels miss {bar} there): '
             + (', '.join(f'`{f}`' for f in flagged) or 'none') + '.',
             f'- **(c)** level-8 bar {bar} as it is.', '',
             'Each class: the head of every token band of its 8-bit cells under each choice (`tables/<choice>.txt` says which '
             'kernels the bars left out and why), then each model\'s arm times on its reference card (mean execution, s): '
             'the 16-bit reference, today\'s choice, and each 8-bit kernel pinned. (a) runs about as the arm of its heads, (b) '
             'as (a) with its flagged layers at 16 bits, (c) as the 16-bit arm where its 8-bit heads are 16-bit kernels.', '',
             'Note: the sparse route\'s own loss against dense FP32 (H3 block 0 at a sparse step: about 0.26 for BF16 Sol) is the '
             'model\'s choice of sparsity, reported, not judged.', '',
             'The flagged layers\' errors, which operand carries the outlier, and their models\' picture pairs beside them: '
             '`real-data.md`; every output with its plain label: `manifest.json`.', '']
    for klass in sorted({k for t in tables.values() for k, _, _ in t}):
        lines += [f'## {klass}', '']
        for layout in ('dense', 'sparse'):
            for o in 'abc':
                lines.append(f"- ({o}) {layout} 8: " + ', '.join(tables[o].get((klass, layout, 8), ['-'])))
        lines.append('')
        for model, by_arm in sorted(times.get(klass, {}).items()):
            text = ', '.join(f'{a} {s} s' for a, s in sorted(by_arm.items(), key=lambda kv: kv[1]))
            d = dense.get(klass, {}).get(model, {})
            if model == 'minimax-h3' and d.get('exact') and d.get('table') and 'table' in by_arm:
                # (c) with dense 8-bit cells at 16 bits: today's run with its four dense steps at the 16-bit arm's pace.
                text += f"; (c) about {round(by_arm['table'] + 4 * (d['exact'] - d['table']), 1)} s (dense steps {d['table']} -> {d['exact']} s)"
            lines.append(f"- {MODEL_OF.get(model, model)}: {text}")
        lines.append('')
    (out / 'options.md').write_text('\n'.join(lines) + '\n')
    print(f'options: (a) {bar_a}, (b) {bar} with {len(flagged)} layers at 16 bits, (c) {bar}; {out / "options.md"}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main(sys.argv))
