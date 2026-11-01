from pathlib import Path
from collections import Counter,defaultdict
import hashlib,json
OUT=Path(__file__).resolve().parent
ROOT=OUT.parent
PAIR=ROOT/'varena-v2-p0-20261031/p3-lifecycle-sparse-leg16-plan/attempt-002/sparse-full-pair-167'
COHORT=ROOT/'xs138-sparse-producer/cohort-9b820-ee247-fc4af-225c2'
sha=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
manifest=json.loads((COHORT/'full-h3/no-piece-memo-provenance.json').read_text())
assert manifest['original_source']=='fc4af2f2e5d075f04bde1d9803cae92a378c84f3' and len(manifest['files'])==43
for file,digest in manifest['files'].items():assert sha(COHORT/'full-h3/no-piece-memo'/file)==digest
request=ROOT/'varena-v2-p0-20261031/h3-plan/request.json';assert sha(request)=='fe62dc81e2830a0a40f967f785dbf3566fa152052c069d0b87bc556b09568341'
inputs=json.loads(request.read_text());assert inputs['duration_s']==15 and inputs['steps']==30 and inputs['seed']==849626408

def arm(name,number,version,source):
 path=PAIR/name/'run-show.json';run=json.loads(path.read_text());assert run['number']==number and run['status']=='completed'
 piece,=[c for c in run['calls'] if c.get('function')=='fl2va_piece'];assert not piece['memoized']
 m=piece['measurements'];assert m['software']['runtime']==version and m['software']['tensorfs']=='0.17.16+dev.ga96c7c3e' and m['software']['tensord_commit']=='616cf254b032e7b6522d3205657a41485db33579'
 assert m['execution']['degree']==1 and m['execution']['gpus'][0]['uuid'].removeprefix('GPU-')=='846689c6-daf3-15b1-580e-f2a78bdf6748'
 assert m['attribution']['steps']['denoise']['count']==30
 invoke=json.loads((PAIR/name/'model-invocation.json').read_text());command=invoke['command'];assert invoke['returncode']==0 and Path(command[command.index('--input')+1])==request
 assert str(COHORT/'full-h3/no-piece-memo/minimax-h3/shot') in command and '--lane=int8' in command and '--gpus=1' in command
 install=json.loads((PAIR/name/'install-invocation.json').read_text());receipt=json.loads((ROOT/f'varena-v2-p0-20261031/runtime-candidate-{source[:8]}.json').read_text());assert receipt['source']==source and receipt['wheel'] in install['command'] and install['returncode']==0
 native=json.loads((ROOT/'xs138-native-cache-candidate-a96c7c3e8/candidate.json').read_text());assert native['wheel_path'] in install['command']
 s=m['memory']['stats']['varena'][0]['stats'];assert s['weight_residency']['streaming_limit_bytes']==2**64-1 and s['weight_residency']['reclaimed_by_reason']['streaming_limit']==0
 assert s['allocator']['ooms']==s['allocator']['context_joins']==s['weight_residency']['context_joins']==0 and not s['weight_residency']['poisoned']
 by_model=defaultdict(Counter);origins=Counter()
 for row in s['trace']:
  origins[row['origin']]+=row['bytes'];by_model[row['set'].rsplit('/',1)[-1]][row['origin']+'_bytes']+=row['bytes'];by_model[row['set'].rsplit('/',1)[-1]]['fills']+=1
 assert origins['store']==s['moved']['store']['bytes'] and origins['baked']==s['moved']['baked']['bytes']
 sites={row['key']:row for row in s['working_splits']};selections=Counter();samples=defaultdict(list)
 for row in sites.values():selections[row['site']]+=row['selections']
 for parent in s['working_phases']:
  for row in parent['split_observations']:samples[(sites[row['key']]['site'],row['choice_id'],sites[row['key']].get('statistics',''))].append(row)
 scopes=[{'site':site,'choice':choice,'statistics':mode,'count':len(rows),'published':sum(bool(r['published'] and not r['pending']) for r in rows),'peak_bytes_max':max(r['peak_bytes'] for r in rows),'backing_peak_min':min(r['backing_peak_bytes'] for r in rows),'backing_peak_max':max(r['backing_peak_bytes'] for r in rows)} for (site,choice,mode),rows in samples.items()]
 assert all(row['count']==row['published'] for row in scopes)
 assert selections['residual-ffn-tail']==1500 and selections['int8-linear-rows']==1450 and selections['projected-heads']==600
 assert selections['sparse-projected-heads']==(900 if name=='candidate' else 0)
 gemms=[o for o in m['observations'] if o['name']=='gemm.selected'];assert len(gemms)==3 and all(o['value']=='cozy-int8' for o in gemms);assert not any(o['name']=='gemm.fallback' for o in m['observations'])
 outputs=[]
 for item in run['output']:
  p=Path(item['path']);assert p.stat().st_size==item['length'] and sha(p)==item['sha256'].removeprefix('sha256:');outputs.append(item)
 result={'run':number,'raw_path':str(path),'raw_sha256':sha(path),'software':m['software'],'models':m['models'],'input':inputs,'request_sha256':sha(request),'invocation':invoke,'install':install,'outputs':outputs,'timing':{'total_ms':run['execution_ms'],'denoise_ms':m['attribution']['stages']['denoise']['total_ms'],'denoise_step_ms':m['attribution']['steps']['denoise']['total_ms'],'denoise_device_ms':m['attribution']['steps']['denoise']['device']['total_ms']},'stages':m['attribution']['stages'],'steps':m['attribution']['steps']['denoise'],'store':s['moved'],'store_by_model':dict(by_model),'working':s['allocator'],'working_cumulative':s['working_allocator'],'weights':s['weight_residency'],'native_selections':dict(selections),'native_scopes':scopes,'splits':s['working_splits'],'legacy_cuts':m['execution']['cuts'],'attention':m['execution']['gpus'][0]['attention'],'attention_selected':[o for o in m['observations'] if o['name']=='attention.selected'],'plans':m['execution']['plan_reports'],'driver':m['memory']['driver'],'prefetch':s['prefetch']}
 (OUT/(name+'-analysis.json')).write_text(json.dumps(result,indent=2)+'\n');(OUT/(name+'-run.json')).write_bytes(path.read_bytes());return result

a=arm('control',25627,'0.36.23+dev.g00f9aab0','00f9aab0b020e5b53c3bd6e96c72788e5317463c')
b=arm('candidate',25678,'0.36.23+dev.g167ffc82','167ffc828e3f84062d43641fcf53dbda54892631')
assert a['models']==b['models']
assert [(r['value'],r['fields']) for r in a['attention_selected']]==[(r['value'],r['fields']) for r in b['attention_selected']]
def clean(v):
 if isinstance(v,dict):return {k:clean(x) for k,x in v.items() if k!='warm_ms'}
 if isinstance(v,list):return [clean(x) for x in v]
 return v
assert clean(a['plans'])==clean(b['plans'])
def delta(x,y):return {'control':x,'candidate':y,'delta':y-x,'percent':None if not x else 100*(y/x-1)}
metrics={k:delta(a['timing'][k],b['timing'][k]) for k in a['timing']}
for origin in ('store','baked'):
 metrics[origin+'_bytes']=delta(a['store'][origin]['bytes'],b['store'][origin]['bytes']);metrics[origin+'_sync_ms']=delta(a['store'][origin]['ns']/1e6,b['store'][origin]['ns']/1e6)
for k in ('allocations','driver_memory_calls','event_records','event_queries','event_waits','ooms','context_joins'):metrics[k]=delta(a['working'][k],b['working'][k])
for k in ('evictions','fills','hits','driver_physical_ooms'):metrics['weight_'+k]=delta(a['weights'][k],b['weights'][k])
metrics['working_physical_ooms_cumulative']=delta(a['working_cumulative']['driver_physical_ooms'],b['working_cumulative']['driver_physical_ooms'])
result={'matched_source_request':True,'same_native_a96':True,'same_worker616':True,'same_fc4_no_piece_memo_source':True,'source_files':43,'metrics':metrics,'native_adoption':{'control':a['native_selections'],'candidate':b['native_selections']},'candidate_sparse_choices':[r for r in b['native_scopes'] if r['site']=='sparse-projected-heads'],'no_worse_gate':False,'limitations':['one serial pair on one GPU','driver before_bytes is the first asynchronous whole-device sample, not a guaranteed pre-Start baseline','physical OOM fields are cumulative snapshots; terminal OOM and context-join counters are zero','source/working reductions do not establish a performance win','quality review is independently scoped to eight paired frames and numerical media metrics']}
(OUT/'pair-metrics.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps({'metrics':metrics,'sparse_choices':result['candidate_sparse_choices']},indent=2))
