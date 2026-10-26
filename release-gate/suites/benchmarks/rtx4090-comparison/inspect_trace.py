"""Read retained real Kineto JSON; rank time/transfer evidence without summing overlap as wall time."""
import argparse
from collections import Counter,defaultdict
import hashlib
import json
from pathlib import Path

p=argparse.ArgumentParser();p.add_argument('trace',type=Path);p.add_argument('--out',type=Path,required=True);a=p.parse_args()
doc=json.loads(a.trace.read_text());events=[e for e in doc['traceEvents'] if e.get('ph')=='X' and e.get('dur',0)>0]
scopes=[e for e in events if e.get('name','').startswith(('cozy.sdxl.request/','cozy.qwen21.request/','comfy.prompt/'))]
if not scopes:scopes=[e for e in events if e.get('cat')=='Trace']
assert scopes,'no explicit request or profiler boundary'
scope=max(scopes,key=lambda e:e['dur']);start,end=scope['ts'],scope['ts']+scope['dur']
def interval(e):return max(start,e['ts']),min(end,e['ts']+e['dur'])
def union(rows):
 merged=[]
 for lo,hi in sorted(rows):
  if hi<=lo:continue
  if merged and lo<=merged[-1][1]:merged[-1][1]=max(merged[-1][1],hi)
  else:merged.append([lo,hi])
 return merged
cats=Counter(e.get('cat','') for e in events)
gpu=[e for e in events if e.get('cat') in ('kernel','gpu_memcpy','gpu_memset') and interval(e)[1]>interval(e)[0]]
kernel_count=sum(e.get('cat')=='kernel' for e in gpu)
cuda_api_count=sum(e.get('cat') in ('cuda_runtime','cuda_driver') and interval(e)[1]>interval(e)[0] for e in events)
incomplete=[]
if not kernel_count:incomplete.append('No GPU kernel events in the selected request scope; CUDA profiling may be unavailable or incomplete.')
if not cuda_api_count:incomplete.append('No CUDA runtime/driver activity in the selected request scope; the CPU/GPU timeline is not established.')
cuda_complete=not incomplete
busy=union([interval(e) for e in gpu]);gaps=[];mark=start
for lo,hi in busy:
 if lo>mark:gaps.append((mark,lo))
 mark=max(mark,hi)
if mark<end:gaps.append((mark,end))
def groups(cat):
 found=defaultdict(list)
 for e in events:
  if e.get('cat')==cat and interval(e)[1]>interval(e)[0]:found[e['name']].append(e)
 return sorted([{'name':name,'count':len(rows),'duration_sum_ms':sum(e['dur'] for e in rows)/1000,
                 'max_ms':max(e['dur'] for e in rows)/1000} for name,rows in found.items()],key=lambda x:x['duration_sum_ms'],reverse=True)
transfers=defaultdict(lambda:{'count':0,'bytes':0,'duration_sum_ms':0.0})
for e in gpu:
 if e['cat']=='gpu_memcpy':
  d=transfers[e['name']];d['count']+=1;d['bytes']+=e.get('args',{}).get('bytes',0);d['duration_sum_ms']+=e['dur']/1000
ranked=[]
for lo,hi in (sorted(gaps,key=lambda x:x[1]-x[0],reverse=True)[:20] if cuda_complete else []):
 if hi-lo<1000:continue
 overlap=[]
 for e in events:
  if e.get('cat') not in ('cuda_runtime','cuda_driver','user_annotation'):continue
  if e is scope:continue
  shared=max(0,min(hi,e['ts']+e['dur'])-max(lo,e['ts']))
  if shared:overlap.append({'name':e['name'],'category':e['cat'],'overlap_ms':shared/1000,'event_duration_ms':e['dur']/1000})
 ranked.append({'start_from_scope_ms':(lo-start)/1000,'duration_ms':(hi-lo)/1000,'overlapping_cpu_scopes':sorted(overlap,key=lambda x:x['overlap_ms'],reverse=True)[:8]})
summary={'trace':str(a.trace),'sha256':hashlib.sha256(a.trace.read_bytes()).hexdigest(),'scope':scope['name'],'scope_wall_ms':(end-start)/1000,'event_categories':dict(cats),'status':'cuda_activity_present' if cuda_complete else 'incomplete_cuda_activity','cuda_activity':{'kernel_count':kernel_count,'runtime_driver_count':cuda_api_count,'incomplete_reasons':incomplete},'gpu_busy_union_ms':sum(hi-lo for lo,hi in busy)/1000 if cuda_complete else None,'gpu_uncovered_ms':sum(hi-lo for lo,hi in gaps)/1000 if cuda_complete else None,'largest_gpu_gaps':ranked,'cpu_operations':groups('cpu_op')[:25],'cpu_annotations':groups('user_annotation')[:20],'kernels':groups('kernel')[:25],'cuda_runtime':groups('cuda_runtime')[:20],'cuda_driver':groups('cuda_driver')[:20],'transfers':dict(transfers),'limitations':['GPU uncovered time is withheld unless actual kernels and CUDA runtime/driver activity are both present. Missing CUDA capture is not GPU starvation.','Activity presence does not prove complete CUPTI capture; inspect profiler warnings and dropped events before causal claims.','GPU uncovered time is only this traced process; corroborate exclusiveGPU telemetry. It includes legitimate CPU tokenization/encoding and is not automatically weight starvation.','Named kernel/API duration sums may overlap and are not added to walltime. GPU busy is interval union across streams.','CPU overlap indicates temporal association, not causation. Inspect correlation/stream dependencies before claiming critical-path cause.','CUDA memcpy bytes are observed requested transfer sizes, not all physicalPCIe traffic or logical weight bytes.','Profiler overhead means these durations cannot replace unprofiled benchmark samples.']}
a.out.write_text(json.dumps(summary,indent=2)+'\n');print(json.dumps({k:summary[k] for k in ('status','scope','scope_wall_ms','cuda_activity','gpu_busy_union_ms','gpu_uncovered_ms','transfers')},indent=2))
