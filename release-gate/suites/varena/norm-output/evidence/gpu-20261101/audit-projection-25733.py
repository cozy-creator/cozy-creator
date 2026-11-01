from pathlib import Path
import hashlib
import json

out=Path('/home/fidika/cozy_v2/outputs/xs138-p2-norm-output-audit')
root=Path('/home/fidika/cozy_v2/outputs/varena-v2-p0-20261031/p3-lifecycle-sparse-leg16-plan/attempt-002/projection-700')
p=root/'projection-result.json'; q=root/'cleanup-result.json'
x=json.loads(p.read_text()); y=json.loads(q.read_text())
expected=json.loads((out/'fixture-700306f32-3043-a96-sorata/input.json').read_text())
assert x['passed'] and y['passed'] and len(x['cases'])==6
for key,wanted in expected.items():
    for actual in (x['provenance'][key],y['provenance'][key]):
        if key=='gpu_uuid':
            assert actual.removeprefix('GPU-').lower()==wanted.removeprefix('GPU-').lower()
        else:
            assert actual==wanted,key
assert x['provenance']['pid']==y['provenance']['pid']==232308
assert {c['case'] for c in x['cases']}=={f'{mode}/{kind}' for mode in ('first-normal','explicit-sharing','actual-learned-sharing') for kind in ('dense','sparse-bootstrap')}
first=[]; shared=[]
for case in x['cases']:
    assert case['relative_l2']==0
    if case['case'].startswith('first-normal/'):
        assert case['bit_exact'] and case['factor_reads']==0
        samples=case['phase']['split_observations']
        assert len(samples)==2 and all(s['published'] and not s['pending'] for s in samples)
        records=case['records']
        linear=[r for r in records if r['site']=='int8-linear-rows']
        assert len(linear)==1 and linear[0]['input_ownership']=='declared_single_use'
        assert case['sibling_count_after'] is None
        first.append({'case':case['case'],'factor_reads':0,'native_children':len(samples),'bit_exact':True,'input_ownership':linear[0]['input_ownership']})
    else:
        assert case['native_selections']==0
        if case['case'].startswith('actual-learned-sharing/'):
            assert case['fresh_group_before'] is None and case['sibling_count']==2
        shared.append({k:v for k,v in case.items()})
for k in ('allocated_bytes','allocated_capacity','mapped_bytes','pending_bytes','pools','pools_bytes','event_handles'):
    assert x['initial'][k]==x['final'][k],k
assert len(y['prior_request_keys_already_retired'])==3 and not any(y['prior_request_keys_already_retired'].values())
result={'passed':True,'run':25733,'cleanup':25734,'pid':232308,'runtime':'3043db98bb44472ddea03ed7c341b12d232d1569','native':'a96c7c3e837314795d0d21b993df1246dc394aed','package':'5d27a1e97b79ce2f98bc8dd07aacfae5833c888b','fixture':'700306f32aa2ecb0f197fb2690922b4226d713ce','actual_seed':x['provenance']['seed'],'first_calls':first,'sharing_cases':shared,'all6_outputs_bit_exact_or_relative_l2_zero':True,'same_pid_retired_keys':3,'baseline':{k:x['final'][k] for k in ('allocated_bytes','mapped_bytes','pending_bytes','pools_bytes','event_handles')},'raw_sha256':{str(f):hashlib.sha256(f.read_bytes()).hexdigest() for f in (p,q)},'scope':'actual 513-token dense and sparse-bootstrap Attention processors with first-use declared ownership and genuine explicit/learned sharing; full H3 performance and rolling sparse projection remain separate'}
(out/'independent-projection-25733-25734.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
