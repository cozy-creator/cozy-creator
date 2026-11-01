"""Independent immutable-run audit; numerical/perceptual evidence is kept separate."""
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path

OUT = Path(__file__).resolve().parent
BASE = OUT.parent/'varena-v2-p0-20261031/p2-heads-leg14-plan/attempt-001'
PACKAGE = OUT.parent/'xs138-p2-heads-artifacts/full-h3'
manifest = json.loads((PACKAGE/'source-provenance.json').read_text())
sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()
assert manifest['source'] == '3476cb1deaf57f6893d4723b86695d088d28556e' and len(manifest['files']) == 40
for file, digest in manifest['files'].items():
    assert sha(PACKAGE/'source'/file) == digest, file


def arm(name, number, runtime, source):
    raw_path = OUT/f'{name}-run.json'
    run = json.loads(raw_path.read_text())
    assert run['number'] == number and run['status'] == 'completed'
    piece, = [c for c in run['calls'] if c.get('function') == 'fl2va_piece']
    assert not piece['memoized']
    m = piece['measurements']
    assert m['software']['runtime'] == runtime
    assert m['software']['tensorfs'] == '0.17.16+dev.gb20f699c'
    assert m['software']['tensord_commit'] == 'afa6f9229153d1a7e93bbf72fc86f5543a4ac21c'
    assert m['execution']['degree'] == 1
    assert m['execution']['gpus'][0]['uuid'].removeprefix('GPU-') == '9e7b1c5d-0754-e081-0ec2-45c94ce5c59c'
    assert m['attribution']['steps']['denoise']['count'] == 30
    invocation = json.loads((BASE/f'heads-h3-{name}-invocation.json').read_text())
    assert invocation['package'] == manifest['source'] and invocation['source_files_verified'] == 40
    assert invocation['runtime_source'] == source
    assert invocation['tensorfs_source'] == 'b20f699cab2416c6ff3d6e458376c29efbcf448a'
    assert invocation['input']['duration_s'] == 15 and invocation['input']['steps'] == 30
    path = Path(invocation['command'][invocation['command'].index('--input')+1])
    assert sha(path) == invocation['input_sha256'] and json.loads(path.read_text()) == invocation['input']
    s = m['memory']['stats']['varena'][0]['stats']
    assert s['weight_residency']['streaming_limit_bytes'] == 2**64-1
    assert s['weight_residency']['reclaimed_by_reason']['streaming_limit'] == 0
    assert s['allocator']['ooms'] == s['allocator']['context_joins'] == 0
    by_model = defaultdict(Counter)
    origins = Counter()
    for row in s['trace']:
        origins[row['origin']] += row['bytes']
        group = by_model[row['set'].rsplit('/', 1)[-1]]
        group[row['origin']+'_bytes'] += row['bytes']
        group['fills'] += 1
    assert origins['store'] == s['moved']['store']['bytes']
    assert origins['baked'] == s['moved']['baked']['bytes']
    sites = {row['key']: row['site'] for row in s['working_splits']}
    selected = Counter()
    samples = defaultdict(list)
    for row in s['working_splits']:
        selected[row['site']] += row['selections']
    for parent in s['working_phases']:
        for row in parent['split_observations']:
            samples[(sites[row['key']], row['choice_id'])].append(row)
    scopes = [{
        'site': site, 'choice': choice, 'count': len(rows),
        'published': sum(r['published'] and not r['pending'] for r in rows),
        'peak_bytes_max': max(r['peak_bytes'] for r in rows),
        'backing_peak_min': min(r['backing_peak_bytes'] for r in rows),
        'backing_peak_max': max(r['backing_peak_bytes'] for r in rows),
    } for (site, choice), rows in samples.items()]
    assert all(row['count'] == row['published'] for row in scopes)
    assert selected['residual-ffn-tail'] == 1500 and selected['int8-linear-rows'] == 1450
    assert selected['projected-heads'] == (480 if name == 'candidate' else 0)
    assert m['execution']['plan_reports']['fl2va_piece.models.base_model']['minimax-h3-glue']['applied']
    gemm = [o for o in m['observations'] if o['name'] == 'gemm.selected']
    assert len(gemm) == 3 and all(o['value'] == 'cozy-int8' for o in gemm)
    assert not any(o['name'] == 'gemm.fallback' for o in m['observations'])
    outputs = []
    for output in run['output']:
        p = Path(output['path'])
        assert p.is_file() and output['sha256'] == 'sha256:'+sha(p)
        outputs.append({'path': str(p), 'sha256': sha(p), 'media_type': output['media_type'], 'bytes': p.stat().st_size})
    result = {
        'run': number, 'raw_sha256': sha(raw_path), 'input_receipt': invocation,
        'software': m['software'], 'models': m['models'], 'memoized': piece['memoized'],
        'outputs': outputs,
        'timing': {'execution_ms': run['execution_ms'],
                   'denoise_host_ms': m['attribution']['stages']['denoise']['total_ms'],
                   'denoise_step_host_ms': m['attribution']['steps']['denoise']['total_ms'],
                   'denoise_device_ms': m['attribution']['steps']['denoise']['device']['total_ms']},
        'stages': m['attribution']['stages'], 'step_series': m['attribution']['steps']['denoise'],
        'source_transfers': {'by_model': dict(by_model), 'by_origin': dict(origins), 'timed_fills': s['moved']},
        'working_request': s['allocator'], 'working_cumulative': s['working_allocator'],
        'weights': s['weight_residency'], 'prefetch': s['prefetch'], 'prefetch_native': {
            k: v for k,v in s['working_allocator'].items() if k.startswith('prefetch_')},
        'native_selection_counts': dict(selected), 'native_scope_choices': scopes,
        'working_splits': s['working_splits'],
        'phase_samples': [{k:v for k,v in row.items() if k != 'split_observations'} for row in s['working_phases']],
        'legacy_cuts': m['execution']['cuts'], 'attention': m['execution']['gpus'][0]['attention'],
        'attention_receipts': [o for o in m['observations'] if o['name'].startswith('attention.')],
        'gpu_driver': m['memory']['driver'], 'glue': m['execution']['plan_reports'],
    }
    (OUT/f'{name}-analysis.json').write_text(json.dumps(result, indent=2)+'\n')
    return result


a = arm('control',25227,'0.36.21+dev.gab491232','ab491232158e3df112c10e77064f237426338d5a')
b = arm('candidate',25264,'0.36.21+dev.gdae97328','dae973282c1938bb15cf542798a0208eebefee1d')
assert a['input_receipt']['input'] == b['input_receipt']['input']
assert a['input_receipt']['input_sha256'] == b['input_receipt']['input_sha256']
assert a['models'] == b['models']
def without_warm_time(value):
    if isinstance(value, dict):
        return {k: without_warm_time(v) for k,v in value.items() if k != 'warm_ms'}
    return value
assert without_warm_time(a['glue']) == without_warm_time(b['glue'])
assert a['attention']['observed'] == b['attention']['observed']
selected = lambda x: [(r['value'],r['fields']) for r in x['attention_receipts'] if r['name']=='attention.selected']
assert selected(a) == selected(b)
def delta(x,y):
    return {'control': x, 'candidate': y, 'delta': y-x, 'percent': None if x==0 else 100*(y/x-1)}
metrics = {key: delta(a['timing'][key], b['timing'][key]) for key in a['timing']}
for key in ('store','baked'):
    metrics[key+'_source_bytes'] = delta(a['source_transfers']['by_origin'].get(key,0), b['source_transfers']['by_origin'].get(key,0))
for key in a['working_request']:
    metrics['request_'+key] = delta(a['working_request'][key], b['working_request'][key])
for key in ('fills','hits','evictions','reclaimed_bytes','ready_bytes','resident_bytes'):
    metrics['weight_'+key] = delta(a['weights'][key],b['weights'][key])
for key in a['weights']['reclaimed_by_reason']:
    metrics['weight_reclaimed_'+key] = delta(a['weights']['reclaimed_by_reason'][key],b['weights']['reclaimed_by_reason'][key])
by_model={name:{key:delta(before.get(key,0),b['source_transfers']['by_model'][name].get(key,0))
                 for key in set(before)|set(b['source_transfers']['by_model'][name])}
          for name,before in a['source_transfers']['by_model'].items()}
result = {'matched_scope': True, 'runs': [25227,25264], 'source_files_verified':40,
          'metrics': metrics, 'by_model_transfers':by_model,
          'native_selection_counts':{'control':a['native_selection_counts'],'candidate':b['native_selection_counts']},
          'native_scope_choices':{'control':a['native_scope_choices'],'candidate':b['native_scope_choices']},
          'kernel_routes_matched': True, 'selected_attention':a['attention']['observed'],
          'observed_no_worse': {'denoise':b['timing']['denoise_host_ms']<=a['timing']['denoise_host_ms'],
                               'store_source_bytes':b['source_transfers']['by_origin']['store']<=a['source_transfers']['by_origin']['store']},
          'gpu_driver':{'control':a['gpu_driver'],'candidate':b['gpu_driver']},
          'scope_limits':['one sequential sample per arm, no statistical confidence interval',
                          'whole-request driver counters do not identify individual head-kernel costs',
                          'store/kept-host counters are not full hardware PCIe/H2D traffic',
                          'same source and processes do not establish equal OS/storage cache state',
                          'this pair uses old-capability allocator ownership, not cooperative P3 proof',
                          'quality requires media/latent review separately; no byte-equality gate']}
(OUT/'pair-metrics.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps({'matched_scope':True,'native_adoption':result['native_selection_counts'],
                  'metrics':metrics,'observed_no_worse':result['observed_no_worse']},indent=2))
