"""The fixed benchmark suite's numbers, stored baselines and table (suites/bench/README.md).
    bench.py table <gate dir>... [--baseline FILE] [--cells FILE] [--known FILE] [--refs DIR] [--no-output]
                                     cell, baseline, candidate, ratio, ComfyUI, verdict; exit 1 on any FAIL
    bench.py baseline <version> <gate dir>... [--comfyui FILE] [--cells FILE]
                                     baselines/<version>.json from those dirs (the previous file's ComfyUI rows kept),
                                     and each cell's reference output into baselines/refs/ (untracked: assets of the
                                     release-gate-baselines release, which `table` fetches into --refs)
    bench.py admit <gate dir> [--baseline FILE]
                                     the pod's reference workload against its GPU type's stored one; exit 1: out of range
    bench.py noise <gate dir>...     each cell's run-to-run spread over the dirs' warm runs
A gate dir is gate.sh's --out after suite `bench` (suites/bench/cells.sh): bench.tsv names each cell's runs
(cell, cold|warm, record), bench-group its group (<sku>x<gpus>), calibrate-*.json the pod's reference workload.
Outputs are judged with numpy and ffmpeg: uv run -q --no-project --with numpy python -I bench.py ..."""
import glob
import json
import math
import os
import re
import statistics
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from runs import Run  # noqa: E402

BENCH = os.path.join(HERE, 'suites', 'bench')
# The owner's bar: a judged time more than 5% over its baseline fails, unless the cell's own measured noise is wider.
BAR = 0.05
# One warm run's relative standard deviation on one pod, at least (compute-bound cells, 30 repeats on kesshouban and
# pasan, 11-07..09: median 0.8%), and what clock scaling leaves between two pods (same build, 3 pods: 1.4%; unscaled 3.2%).
SD_FLOOR, POD_SD, POD_SD_UNSCALED = 0.008, 0.014, 0.032
D2 = {2: 1.128, 3: 1.693, 4: 2.059, 5: 2.326}  # a range of n normal draws, in standard deviations
JUDGED = ('steady_ms', 'total_ms')
# A reference workload this far from its GPU type's stored one is another class of pod (kesshouban to pasan: 7%).
POD_RANGE = {'attention_ms': 0.12, 'conv_ms': 0.12, 'gemm_ms': 0.12, 'h2d_gbps': 0.35, 'disk_gbps': 0.6}
DRIFT = 0.03  # the reference workload after the cells against before them
REPO, TAG = 'cozy-creator/cozy-creator', 'release-gate-baselines'
MEDIA = ('video', 'image', 'audio')  # a cell's judged output: its first of the first kind it has


def metrics(run, memavail=()):
    """One run's numbers, from its record (and the pod's MemAvailable log, when the gate kept one)."""
    rec, s = run.record, run.steps
    rows = [r for m in run.measurements for r in (m.get('memory') or {}).get('driver') or []]
    stats = [v.get('stats') or {} for m in run.measurements for v in ((m.get('memory') or {}).get('stats') or {}).get('varena') or []]
    res = [x.get('weight_residency') or {} for x in stats]
    transport = next((o['transport'] for o in walk(rec) if isinstance(o.get('transport'), dict) and o['transport'].get('route')), None)
    clocks = [r['sm_mhz_mean'] for r in rows if r.get('sm_mhz_mean')]
    began, ended = rec.get('created_at_ms'), rec.get('settled_at_ms')
    low = min((kb for at, kb in memavail if began and ended and began / 1000 - 5 <= at <= ended / 1000 + 5), default=None)
    vram = run.vram
    return {
        'run': run.number, 'status': run.status, 'reason': run.reason[:200],
        'total_ms': ended - began if began and ended else None, 'execution_ms': run.execution_ms,
        'step1_ms': round(s[0]) if s else None, 'to_step1_ms': run.to_step1_ms,
        'steady_ms': round(sum(s[1:]) / len(s[1:]), 1) if len(s) > 1 else None,
        'denoise_ms': round(sum(s)) if s else None, 'steps': len(s),
        'vram_mib': vram and vram['peak'], 'host_low_mib': low and low // 1024,
        'fills': sum(x.get('fills') or 0 for x in res), 'evictions': sum(x.get('evictions') or 0 for x in res),
        'refilled_gib': round(sum((x.get('working_allocator') or {}).get('weights_filled') or 0 for x in stats) / 2**30, 1),
        'replays': run.replays, 'graphs': run.graphs, 'gemm': run.gemm, 'attention': run.attention,
        'transport': transport and {'route': transport.get('route'), 'degree': transport.get('degree'),
                                    'mapped': (transport.get('check') or {}).get('mapped'), 'exact': (transport.get('check') or {}).get('exact')},
        'clock_mhz': round(statistics.median(clocks)) if clocks else None,
        'clock_limit_mhz': next((r['sm_mhz_limit'] for r in rows if r.get('sm_mhz_limit')), None),
        'throttled': max((r.get('throttled_share') or 0 for r in rows), default=None),
        'throttles': sorted({t for r in rows for t in r.get('throttles') or []}),
        'temperature_c': max((r.get('temperature_max_c') or 0 for r in rows), default=None),
        'power_w': max((r.get('power_max_w') or 0 for r in rows), default=None),
    }


def walk(o):
    if isinstance(o, dict):
        yield o
        for v in o.values():
            yield from walk(v)
    elif isinstance(o, list):
        for v in o:
            yield from walk(v)


def output_of(d, run, judged):
    """The run's judged output: its file among the gate's project files, by digest, and what it is alone."""
    import glob as g
    outs = sorted((o for o in run.record.get('output') or [] if (o.get('media_type') or '').split('/')[0] in MEDIA and o.get('sha256')),
                  key=lambda o: MEDIA.index(o['media_type'].split('/')[0]))
    if not outs:
        return None  # a text answer: nothing to look at
    digest = outs[0]['sha256'].split(':')[-1]
    files = [f for sub in ('project/files', 'proj/files', '../proj/files', '../project/files') for f in g.glob(os.path.join(d, sub, digest + '.*'))]
    if not judged:
        return {'sha256': digest}
    if not files:
        return {'sha256': digest, 'faults': ['its output is not among the gate\'s files']}
    from suites.bench import output
    said = output.alone(files[0])
    return {'sha256': digest, 'file': files[0], 'alone': said, 'faults': output.faults(said)}


def summary(cold, warm):
    """A cell: its cold run, the median of its warm runs, and one warm run's relative spread (from their range)."""
    done = [m for m in warm if m['status'] == 'completed']
    mid, spread = {}, {}
    for k in ('total_ms', 'execution_ms', 'step1_ms', 'steady_ms', 'denoise_ms', 'vram_mib', 'host_low_mib', 'fills', 'evictions',
              'refilled_gib', 'replays', 'clock_mhz', 'throttled', 'temperature_c'):
        vals = [m[k] for m in done if m.get(k) is not None]
        if vals:
            mid[k] = statistics.median(vals)
            if k in JUDGED and len(vals) > 1 and mid[k]:
                spread[k] = round((max(vals) - min(vals)) / D2[min(len(vals), 5)] / mid[k], 4)
    last = done[-1] if done else {}
    shown = [m for m in ([cold] if cold else []) + done if m.get('output')]
    return {'cold': cold, 'warm': mid, 'spread': spread, 'n': len(done), 'runs': [m['run'] for m in warm],
            # The cold run has the cell's fixed seed: its output is the one held against the reference.
            'output': (cold or {}).get('output') if cold and cold['status'] == 'completed' else None,
            'output_faults': [f"run {m['run']} output: {w}" for m in shown for w in m['output'].get('faults', [])],
            'failed': [f"run {m['run']} {m['status']}: {m['reason']}" for m in ([cold] if cold else []) + warm if m['status'] != 'completed'],
            **{k: last.get(k) for k in ('graphs', 'gemm', 'attention', 'transport', 'throttles', 'clock_limit_mhz')}}


def read_gate(d, judged=True):
    """A gate dir's bench cells, pod and reference workload."""
    d = d.rstrip('/')
    text = lambda name: open(os.path.join(d, name)).read().strip() if os.path.exists(os.path.join(d, name)) else None
    memavail = [(int(a), int(b)) for a, b in (l.split()[:2] for l in open(os.path.join(d, 'memavail.log')) if l.strip())] \
        if os.path.exists(os.path.join(d, 'memavail.log')) else []
    by = {}
    for line in open(os.path.join(d, 'bench.tsv')):
        cell, role, record = line.rstrip('\n').split('\t')[:3]
        path = record if record.endswith('.json') else record + '.json'
        run = Run.read(path if os.path.isabs(path) else os.path.join(d, path))
        m = metrics(run, memavail) if run else {'run': None, 'status': 'no record', 'reason': record}
        if run and run.status == 'completed':
            m['output'] = output_of(os.path.dirname(os.path.abspath(path)) if os.path.isabs(path) else d, run, judged)
        by.setdefault(cell, {'cold': None, 'warm': []})
        if role == 'cold':
            by[cell]['cold'] = m
        else:
            by[cell]['warm'].append(m)
    ref = {}
    for when in ('before', 'after'):
        run = Run.read(os.path.join(d, f'calibrate-{when}.json'))
        if run and run.status == 'completed' and isinstance(run.result, dict):
            ref[when] = json.loads(run.result['measured'])
    return {'dir': d, 'group': text('bench-group'), 'pod': text('machine'), 'label': text('label'), 'reference': ref,
            'cells': {name: summary(c['cold'], c['warm']) for name, c in by.items()}}


def cells_file(path=None):
    """suites/bench/cells.tsv: name, group, cap, warm runs, seed, target, inputs, before, beats, like, route, guards."""
    keys = ('name', 'group', 'cap', 'warm', 'seed', 'target', 'args', 'before', 'beats', 'like', 'route', 'guards')
    rows = [dict(zip(keys, l.rstrip('\n').split('\t') + [''] * len(keys))) for l in open(path or os.path.join(BENCH, 'cells.tsv'))
            if l.strip() and not l.startswith('#')]
    return [{k: ('' if v == '-' else v) for k, v in r.items()} for r in rows]


def version_key(path):
    return [int(x) for x in re.findall(r'\d+', os.path.basename(path))]


def newest_baseline():
    files = sorted(glob.glob(os.path.join(HERE, 'baselines', '*.json')), key=version_key)
    return files[-1] if files else None


def sd_of_median(cell, key):
    return max(cell['spread'].get(key, 0), SD_FLOOR) * (1.2533 if cell['n'] > 2 else 1) / math.sqrt(max(cell['n'], 1))


def threshold(base, cand, key, pod_sd):
    """How far over its baseline a judged time may be: the bar, or three standard deviations of the ratio of the two
    medians (each cell's own measured spread, and what stays between two pods), whichever is wider."""
    return max(BAR, 3 * math.sqrt(sd_of_median(base, key) ** 2 + sd_of_median(cand, key) ** 2 + pod_sd ** 2))


def ref_file(refs, entry):
    """A stored reference output, fetched from the baselines release when --refs lacks it."""
    import subprocess
    path = os.path.join(refs, entry['asset'])
    if not os.path.exists(path):
        os.makedirs(refs, exist_ok=True)
        subprocess.run(['gh', 'release', 'download', TAG, '-R', REPO, '-D', refs, '-p', entry['asset']], capture_output=True)
    return path if os.path.exists(path) else None


def judge_output(d, c, b, others, refs):
    """Why a cell's output fails: alone, against its stored reference, and against the one-GPU output of its request."""
    from suites.bench import output
    why, o = list(c['output_faults']), c['output']
    if not o or not o.get('file'):
        return why
    kind = o['alone']['kind']
    stored = (b or {}).get('output') or {}
    if stored.get('alone'):
        why += [w for w in output.faults(o['alone'], stored['alone']) if w not in o['faults']]
        ref = ref_file(refs, stored)
        why += output.differs(kind, output.pair(ref, o['file'])) if ref else [f"reference output {stored['asset']} not found in {refs} or the {TAG} release"]
    if d['like']:
        other = next((g['cells'][d['like']]['output'] for g in others if d['like'] in g['cells'] and (g['cells'][d['like']]['output'] or {}).get('file')), None)
        if other:
            why += output.differs(kind, output.pair(other['file'], o['file']), f"{d['like']}'s output (same request and seed)")
        else:
            why.append(f"no {d['like']} output of this candidate to hold it against")
    return why


def judge(defs, cand, base, known, same_pod=False, others=(), refs=None):
    """Each cell of the group's list against the baseline: [(name, baseline, candidate, ratios, comfy, verdict, why)]."""
    rows = []
    bcells, comfy = (base or {}).get('cells', {}), (base or {}).get('comfyui', {})
    for d in defs:
        name, c, b = d['name'], cand['cells'].get(d['name']), bcells.get(d['name'])
        why, notes, ratios = [], [], {}
        if not c or (not c['n'] and not c['failed']):
            rows.append((name, b, c, ratios, comfy.get(name), 'FAIL', ['not run'], notes)); continue
        why += c['failed']
        if refs is not None:
            why += judge_output(d, c, b, others, refs)
        w = c['warm']
        cap = int(d['cap']) * 1024 if d['cap'] else None
        if cap and w.get('vram_mib') and w['vram_mib'] > cap:
            why.append(f"peak VRAM {w['vram_mib'] / 1024:.2f} GiB over its {d['cap']} GiB limit")
        if d['route'] and c['n']:
            t = c.get('transport') or {}
            if t.get('route') != d['route'] or (d['route'] != 'nccl' and not (t.get('mapped') and t.get('exact'))):
                why.append(f"transport {t.get('route')} mapped={t.get('mapped')} exact={t.get('exact')}, not {d['route']}")
        one = cand['cells'].get(d['beats']) if d['beats'] else None
        if d['beats'] and c['n']:
            if not one or not one['n']:
                why.append(f"no {d['beats']} run to beat")
            else:
                for k in ('total_ms', 'denoise_ms'):
                    if w.get(k) and one['warm'].get(k) and w[k] >= one['warm'][k]:
                        why.append(f"{k[:-3]} {w[k] / 1000:.1f} s does not beat {d['beats']}'s {one['warm'][k] / 1000:.1f} s")
        clock = 1.0
        if b and b['n'] and c['n']:
            bw = b['warm']
            scaled = bool(w.get('clock_mhz') and bw.get('clock_mhz'))
            clock = w['clock_mhz'] / bw['clock_mhz'] if scaled and not same_pod else 1.0
            pod_sd = 0 if same_pod else POD_SD if scaled else POD_SD_UNSCALED
            if not scaled and not same_pod:
                notes.append('no GPU clock on one side: judged unscaled, at a wider bound')
            for k in JUDGED:
                if not w.get(k) or not bw.get(k):
                    continue
                # The pod's clock scales GPU time only: a step wholly, a total by its denoise.
                gpu = w[k] if k == 'steady_ms' else min(w.get('denoise_ms') or 0, w[k])
                raw, cal = w[k] / bw[k], (w[k] - gpu + gpu * clock) / bw[k]
                limit = threshold(b, c, k, pod_sd)
                ratios[k] = (raw, cal, limit)
                if cal > 1 + limit and (k != 'total_ms' or w[k] - bw[k] > 300):
                    why.append(f"{k[:-3]} {cal:.2f}x its baseline (raw {raw:.2f}x; at most {1 + limit:.2f}x)")
            if (bw.get('fills') or 0) and (w.get('refilled_gib') or 0) > 1.5 * (bw.get('refilled_gib') or 0) + 1:
                notes.append(f"refilled {w['refilled_gib']:.0f} GiB a call, baseline {bw.get('refilled_gib', 0):.0f}")
            if (b.get('gemm'), b.get('attention')) != (c.get('gemm'), c.get('attention')):
                notes.append(f"kernels {b.get('gemm')} / {b.get('attention')} -> {c.get('gemm')} / {c.get('attention')}")
        elif c['n']:
            notes.append('no baseline: this run starts it')
        x = comfy.get(name)
        if x and x.get('total_s') and b and b['n'] and c['n'] and w.get('total_ms'):
            ours = (w['total_ms'] - (w.get('denoise_ms') or 0) * (1 - clock)) / 1000
            if b['warm']['total_ms'] / 1000 < x['total_s'] <= ours:
                why.append(f"loses to ComfyUI ({ours:.1f} s vs {x['total_s']:.1f} s) where {base.get('version')} won ({b['warm']['total_ms'] / 1000:.1f} s)")
        if c.get('throttles') and 'sw_thermal' in c['throttles']:
            notes.append(f"thermal throttling ({w.get('temperature_c')} C, throttled {w.get('throttled', 0):.0%} of the time)")
        issue = next((k for k in known if k['cell'] == name and k.get('group', cand['group']) == cand['group']), None)
        verdict = 'PASS' if not why else 'KNOWN' if issue else 'FAIL'
        if issue:
            notes.append(f"known issue {issue['issue']}: {issue['reason']}" + ('' if why else ' (passes now: remove it)'))
        rows.append((name, b, c, ratios, x, verdict if c['n'] or why else 'FAIL', why, notes))
    return rows


def pod_check(cand, base):
    """The pod's reference workload against its GPU type's stored one, and after the cells against before."""
    out, ref, stored = [], cand['reference'], ((base or {}).get('reference') or {})
    if not ref.get('before'):
        return ['no reference workload on this pod']
    for k, by in POD_RANGE.items():
        now, was = ref['before'].get(k), stored.get(k)
        if now and was:
            off = now / was - 1
            slower = off > by if k.endswith('_ms') else -off > by
            if slower:
                out.append(f"{k} {now:.3g} vs this GPU type's {was:.3g} ({off:+.0%}, at most {by:.0%} off)")
    after = ref.get('after')
    for k in ('attention_ms', 'conv_ms', 'gemm_ms'):
        if after and after.get(k) and ref['before'].get(k) and abs(after[k] / ref['before'][k] - 1) > DRIFT:
            out.append(f"{k} drifted {after[k] / ref['before'][k] - 1:+.1%} over the run ({ref['before'].get('temperature_c')} -> {after.get('temperature_c')} C)")
    return out


def fmt(cell):
    if not cell or not cell['n']:
        return 'failed' if cell and cell['failed'] else '-'
    w = cell['warm']
    f = lambda k, div=1000, digits=2: '-' if w.get(k) is None else f'{w[k] / div:.{digits}f}'
    return f"{f('steady_ms')} / {f('total_ms', digits=1)} / {f('vram_mib', 1024, 1)}"


def table(gates, base, defs, known, refs=None):
    fails, lines = [], []
    for g in gates:
        group = ((base or {}).get('groups') or {}).get(g['group'])
        same = bool(group and group.get('pod') and group['pod'] == g['pod'] and group.get('dir') == g['dir'])
        rows = judge([d for d in defs if d['group'] == g['group']], g, group and {**group, 'version': base['version']}, known, same, gates, refs)
        lines += [f"\n### {g['group']} on {g['pod']}: {g['label']} against {base['version'] if group else 'no baseline'}"
                  f"{' (' + group['pod'] + ')' if group else ''}", '',
                  '| cell | baseline steady s / total s / peak GiB | candidate | steady x | total x | ComfyUI total s | verdict |', '|---|---|---|---|---|---|---|']
        for name, b, c, ratios, x, verdict, why, notes in rows:
            r = lambda k: '-' if k not in ratios else f'{ratios[k][1]:.2f} (raw {ratios[k][0]:.2f}, limit {1 + ratios[k][2]:.2f})'
            lines.append(f"| {name} | {fmt(b)} | {fmt(c)} | {r('steady_ms')} | {r('total_ms')} | {x['total_s'] if x and x.get('total_s') else '-'} |"
                         f" {verdict}{': ' + '; '.join(why) if why else ''}{' (' + '; '.join(notes) + ')' if notes else ''} |")
            if verdict == 'FAIL':
                fails.append(f"{g['group']} {name}: {'; '.join(why)}")
        for p in pod_check(g, group):
            lines.append(f'\npod {g["pod"]}: {p}')
    return lines, fails


def main(argv):
    opt = lambda flag: argv[argv.index(flag) + 1] if flag in argv else None
    flags = {i for f in ('--baseline', '--cells', '--known', '--comfyui', '--refs') if f in argv for i in (argv.index(f), argv.index(f) + 1)}
    args = [a for i, a in enumerate(argv) if i not in flags]
    verb, rest = args[0], args[1:]
    defs = cells_file(opt('--cells'))
    known_file = opt('--known') or os.path.join(BENCH, 'known-issues.json')
    known = json.load(open(known_file)) if os.path.exists(known_file) else []
    base_file = opt('--baseline') or newest_baseline()
    base = json.load(open(base_file)) if base_file and os.path.exists(base_file) else None
    if verb == 'table':
        judged = '--no-output' not in argv
        refs = opt('--refs') or os.path.join(HERE, 'baselines', 'refs')
        lines, fails = table([read_gate(d, judged) for d in rest if d != '--no-output'], base or {'version': None}, defs, known, refs if judged else None)
        if not judged:
            lines.append('\noutputs NOT judged (--no-output): no verdict for a release')
        print('\n'.join(lines))
        print('\n' + '\n'.join(f'FAIL {f}' for f in fails) if fails else '')
        print(f"VERDICT {'FAIL' if fails else 'PASS'} ({len(fails)} failing cells)")
        return 1 if fails else 0
    if verb == 'baseline':
        gates = [read_gate(d) for d in rest[1:]]
        out = {'version': rest[0], 'groups': {}}
        refs = os.path.join(HERE, 'baselines', 'refs')
        os.makedirs(refs, exist_ok=True)
        for g in gates:
            for name, c in g['cells'].items():
                o = c.get('output')
                if o and o.get('file'):
                    import shutil
                    asset = f"bench-{rest[0]}-{g['group']}-{name}{os.path.splitext(o['file'])[1]}"
                    shutil.copyfile(o['file'], os.path.join(refs, asset))
                    c['output'] = {'asset': asset, 'sha256': o['sha256'], 'alone': o['alone']}
            out['groups'][g['group']] = {'pod': g['pod'], 'label': g['label'], 'dir': g['dir'], 'reference': g['reference'].get('before') or {},
                                         'cells': g['cells'], 'comfyui': ((base or {}).get('groups') or {}).get(g['group'], {}).get('comfyui', {})}
        if opt('--comfyui'):
            for group, cells in json.load(open(opt('--comfyui'))).items():
                out['groups'].setdefault(group, {'cells': {}})['comfyui'] = cells
        path = os.path.join(HERE, 'baselines', rest[0] + '.json')
        os.makedirs(os.path.dirname(path), exist_ok=True)
        json.dump(out, open(path, 'w'), indent=1, sort_keys=True)
        print(path)
        return 0
    if verb == 'admit':
        g = read_gate(rest[0])
        bad = pod_check(g, ((base or {}).get('groups') or {}).get(g['group']))
        print('\n'.join(bad) or f"pod {g['pod']} in range")
        return 1 if bad else 0
    if verb == 'noise':
        for g in (read_gate(d) for d in rest):
            for name, c in g['cells'].items():
                print(f"{g['group']}\t{g['pod']}\t{name}\tn={c['n']}\t" + '\t'.join(f"{k[:-3]} ±{c['spread'].get(k, 0):.1%}" for k in JUDGED))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
