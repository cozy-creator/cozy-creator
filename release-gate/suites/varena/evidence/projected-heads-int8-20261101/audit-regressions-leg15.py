import hashlib
import json
from pathlib import Path

OUT = Path(__file__).resolve().parent
BASE = OUT.parent/'varena-v2-p0-20261031/p3-shortage-int8-leg15-plan/attempt-001'
sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()
uuid = '18d960a8-2283-19c9-c060-ef8a22fabe35'
records = {}
for kind, run_id, cleanup_id, count, template_name in (
    ('linear', 25317, 25318, 6, 'standalone-linear-input-3daaaeda9-unbound.json'),
    ('tail', 25320, 25321, 5, 'complete-tail-input-3daaaeda9-unbound.json'),
):
    raw = BASE/f'int8-{kind}-regression-{run_id}.json'
    result_path = BASE/f'int8-{kind}-regression-result.json'
    cleanup_raw = BASE/f'int8-{kind}-cleanup-{cleanup_id}.json'
    cleanup_path = BASE/f'int8-{kind}-cleanup-result.json'
    run, result, cleanup_run, cleanup = [json.loads(p.read_text()) for p in (raw,result_path,cleanup_raw,cleanup_path)]
    for data, expected in ((run,run_id),(cleanup_run,cleanup_id)):
        assert data['number']==expected and data['status']=='completed' and data['result']['passed']
        assert data['machine']=='hitoshiki'
    assert json.loads(run['result']['detail'])==result
    assert json.loads(cleanup_run['result']['detail'])==cleanup
    assert result['passed'] and cleanup['passed'] and len(result['cases'])==count
    template=json.loads((OUT/template_name).read_text())
    for key,value in template.items():
        if key!='gpu_uuid':assert result['provenance'][key]==value,(kind,key)
    assert result['provenance']['gpu_uuid'].removeprefix('GPU-')==uuid
    assert cleanup['provenance']['pid']==cleanup['provenance']['expected_pid']==result['provenance']['pid']
    assert set(cleanup['prior_request_keys_already_retired'])==set(result['request_owned_keys'])
    assert not any(cleanup['prior_request_keys_already_retired'].values())
    for field in ('allocated_bytes','allocated_capacity','mapped_bytes','pending_bytes','pools','pools_bytes','event_handles'):
        assert result['final'][field]==result['initial'][field],(kind,field)
    numeric=[]
    for case in result['cases']:
        if 'relative_l2' in case:
            assert case['relative_l2']==0
            numeric.append(case['relative_l2'])
        if 'exact' in case:assert case['exact']
        if 'refusal' in case:
            refusal=case['refusal'];assert refusal['no_retry'] and refusal['allocation_delta']==0
            assert refusal['required_bytes']>refusal['available_bytes']
            assert case['phase']['invalidated'] and not case['phase']['published']
            if 'output_unchanged' in case:assert case['output_unchanged']
        elif 'phase' in case:
            samples=case['phase']['split_observations']
            if case.get('baked_hook') is False:
                assert not samples and case['factor_reads'] > 0  # deliberate unsupported native contract
            else:
                assert samples and all(s['published'] for s in samples)
    queries=result['query_execution']
    if isinstance(queries,dict):queries=[q for group in queries.values() for q in group]
    assert all(q['relative_l2']==0 for q in queries)
    if kind=='tail':assert result['hook_factor_reads']==0
    records[kind]={'passed':True,'run':run_id,'cleanup':cleanup_id,'cases':count,
                   'pid':result['provenance']['pid'],'numeric_cases':len(numeric),
                   'max_relative_l2':max(numeric),'actual_query_checks':len(queries),
                   'query_max_relative_l2':max(q['relative_l2'] for q in queries),
                   'baseline_allocated_bytes':result['initial']['allocated_bytes'],
                   'final_allocated_bytes':result['final']['allocated_bytes'],
                   'baseline_mapped_bytes':result['initial']['mapped_bytes'],
                   'final_mapped_bytes':result['final']['mapped_bytes'],
                   'request_keys_retired':result['request_owned_keys'],
                   'files':{str(p):sha(p) for p in (raw,result_path,cleanup_raw,cleanup_path)}}
receipt={'passed':True,'runtime':'0.36.22+dev.g3daaaeda','tensorfs':'0.17.16+dev.g4dcf2618',
         'gpu_uuid':uuid,'regressions':records,
         'scope':'standalone/Tail helpers with qualified phase-generation forwarding; fixture-owned allocations return to nonzero initial baseline, native leaf observations publish, and same-process request keys retire; not a Heads or full-model performance verdict'}
(OUT/'regression-audit-leg15.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps({k:{a:b for a,b in v.items() if a!='files'} for k,v in records.items()},indent=2))
