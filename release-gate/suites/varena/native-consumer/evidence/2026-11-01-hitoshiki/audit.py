import hashlib
import json
import sys
from pathlib import Path

ROOT=Path(__file__).parent
OUT=Path(__file__).parent
NATIVE='c3bf579ae82b42b254aa30fe54c609fc8e89afcb'
HELPER='30c09e4b2fb1ac0e95ddb028deb766a168606536'
BINARY='351d9fcbb8b36442191eedb2553ea0edc4b0daaaf10393258ad598ce18209476'
UUID='GPU-18d960a8-2283-19c9-c060-ef8a22fabe35'

def audit(path):
 data=json.loads(path.read_text())
 checks=[]
 def check(ok,description):
  checks.append({'passed':bool(ok),'check':description})
 summary={'source':str(path),'sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'case':data.get('case'),'fixture_passed':data.get('passed'),'checks':checks}
 check(data.get('passed') is True,'fixture returned a positive result')
 check(data.get('all_owned_children_exited') is True,'all owned native child exits observed')
 check(not data.get('cleanup_errors'),'no native child cleanup errors')
 prov=data.get('provenance',{})
 check(prov.get('native_source')==NATIVE and prov.get('helper_source')==HELPER and prov.get('helper_sha256')==BINARY,'exact corrected native/helper/binary cohort')
 check(prov.get('gpu_uuid')==UUID,'exact Hitoshiki device UUID')
 children={c['name']:c for c in data.get('children',[])}
 summary['children']=[]
 for name,c in children.items():
  check(c.get('executed_binary_sha256')==BINARY,f'{name}: executed ELF digest')
  maps=c.get('maps') or {}
  check(maps.get('python_loaded') is False and maps.get('torch_loaded') is False,f'{name}: parent observed no Python/Torch maps')
  check(maps.get('cuda_loaded') is (c['mode']!='watch'),f'{name}: expected CUDA mapping boundary')
  rows=c.get('rows',[])
  libs=[r['libraries'] for r in rows if 'libraries' in r]
  check(bool(libs) and all(not l['python_loaded'] and not l['torch_loaded'] for l in libs),f'{name}: native process repeated map checks')
  sent=[r['command'] for r in c.get('sent',[])]
  check(not any(r['op'] in ('ask','service','withdraw','clear_want') for r in sent),f'{name}: controller issued no manual cooperation action')
  deaths=[s for s in c.get('signals',[]) if s['reason']=='exact_observed_waiting_donor']
  closed=[r for r in rows if r.get('event')=='closed']
  if deaths:
   check(c['mode']=='manual' and c['returncode']==-9 and len(deaths)==1,f'{name}: exactly owned manual donor kill')
  else:
   check(c['returncode']==0,f'{name}: clean native exit')
   if c['mode']!='watch':
    check(len(closed)==1 and all(closed[0]['stats'][v]==0 for v in ('live','mapped','pending')),f'{name}: zero working/live/mapped/pending at close')
  summary['children'].append({'name':name,'mode':c['mode'],'pid':c['pid'],'birth':c.get('start_ticks'),'exit':c['returncode'],'closed_stats':closed[0]['stats'] if closed else None,'signals':c.get('signals',[])})
 detail=data.get('detail',{})
 case=data.get('case')
 if case=='smoke_case':
  cases=detail['cases'];check([c['mode'] for c in cases]==['auto','manual','plain'],'all three native modes executed')
  check(all(c['full_readback']['ok'] and c['full_readback']['result']['bytes_verified']==64*2**20 for c in cases),'all three modes fully read back 64MiB')
  summary['readback_bytes']={c['mode']:c['full_readback']['result']['bytes_verified'] for c in cases}
 elif case=='death_case':
  marker=detail['waiting_marker'];death=detail['owned_death'];initial=detail['initial_allocation'];watch=detail['watch_ready']
  requester=children['requester'];donor=children['donor']
  ready={n:next(r for r in c['rows'] if r.get('event')=='ready') for n,c in children.items() if c['mode']!='watch'}
  check(marker['donor']==ready['donor']['row'] and marker['requester']==ready['requester']['row'],'Waiting marker has exact donor/requester RowIds')
  check(marker['pid']==donor['pid'] and marker['start_ticks']==donor['start_ticks'],'Waiting marker matches owned donor PID/birth')
  check(death['identity']['pidfd_live'] and death['identity']['pidfd_pid']==donor['pid'],'live owned pidfd verified before kill')
  begin=next(r['at_ns'] for r in requester['rows'] if r.get('event')=='allocation_begin' and r['id']==initial['id'])
  end=next(r['at_ns'] for r in requester['rows'] if r.get('event')=='allocation_end' and r['id']==initial['id'])
  check(watch['at_ns']<begin<=marker['at_ns']<=death['before_ns']<=death['after_ns']<end,'watch ready precedes allocation; exact Waiting precedes kill inside actual allocation')
  recovery=initial['owner']['last_recovery']
  check(marker['completed']==marker['pending']==recovery['completed_bytes']==0,'dead donor was credited zero completed bytes')
  check(initial['stats']['driver_physical_ooms']>0 and initial['owner']['recoveries']>0,'real driver exhaustion entered automatic recovery')
  check(detail['pressure']['full_readback']['result']['bytes_verified']==detail['pressure']['requested_filler_bytes'],'pressure filler read back every real byte')
  check(detail['requester_cleanup']['settled']['result']['want']==0,'automatic requester demand settled')
  if initial['ok']:
   check(detail['full_readback']['result']['bytes_verified']==prov['request_bytes'],'initial request has complete physical readback')
  else:
   check(initial['error_class']=='capacity','initial physical exhaustion retained as typed capacity result')
  summary['death']={'round':marker['round'],'initial_ok':initial['ok'],'initial_outcome':detail['initial_outcome'],'recovery':recovery,'initial_stats':initial['stats'],'driver':initial['driver'],'physical_readback_proven':detail['physical_readback_proven'],'automatic_episode_progress':detail['automatic_episode_progress'],'filler_bytes':detail['pressure']['requested_filler_bytes'],'interval_ns':[begin,end],'watch_ready_ns':watch['at_ns'],'waiting_ns':marker['at_ns'],'kill_ns':[death['before_ns'],death['after_ns']]}
 elif case=='simultaneous_case':
  intervals=detail['actual_allocation_intervals_ns'];replies=detail['replies'];successes=[r for r in replies if r['ok']]
  check(max(x[0] for x in intervals)<min(x[1] for x in intervals),'actual allocation call intervals overlap')
  check(bool(successes),'at least one automatic consumer made real allocation/readback progress')
  check(all(r['result']['bytes_verified']==prov['simultaneous_bytes'] and r['result']['freed'] for r in successes),'all successful operations read back and freed all requested bytes')
  check(all(r['ok'] or r['error_class']=='capacity' for r in replies),'failures remain typed physical capacity outcomes')
  check(all(x['settled']['result']['want']==0 and x['settled']['owner']['active_recoveries']==0 and x['settled']['owner']['pending_withdrawals']==0 for x in detail['final_owner_rows']),'both native demands/recovery/withdrawals settled without controller mutation')
  summary['simultaneous']={'intervals_ns':intervals,'outcomes':detail['outcomes'],'progress':detail['progress_qualified'],'filler_bytes':detail['pressure']['requested_filler_bytes'],'replies':[{'ok':r['ok'],'stats':r['stats'],'owner':r['owner'],'result':r['result'],'error':r['error']} for r in replies]}
 elif case=='lifetime_case':
  fork=detail['post_cuda_fork']['result'];capture=detail['capture_close']['result']
  check(fork['facts_inherited_rejected'] and fork['close_inherited_rejected'] and fork['child_exit']==0 and fork['parent_owner_active'],'actual post-CUDA fork has exact inherited refusals and parent survives')
  check(capture['actual_cuda_capture'] and capture['close_refused'] and not capture['active'] and capture['stopping'] and not capture['retired_during_capture'] and capture['retired_after_capture'],'actual capture refuses close until capture ends')
  check(capture['working_driver_calls_before']==capture['working_driver_calls_after'] and capture['live_before']==capture['live_after'] and capture['mapped_before']==capture['mapped_after'],'product working-driver calls/live/mapped unchanged during defined capture interval')
  check(detail['after_fork_readback']['result']['bytes_verified']==detail['after_capture_readback']['result']['bytes_verified']==2*2**20,'full 2MiB canary survives both lifetime probes')
  summary['lifetime']={'fork':fork,'capture':capture}
 else:check(False,'recognized native fixture case')
 summary['independent_passed']=all(c['passed'] for c in checks)
 if 'error' in data:summary['error']=data['error']
 target=OUT/(path.stem+'-review.json');target.write_text(json.dumps(summary,indent=2)+'\n')
 print(json.dumps({'file':path.name,'case':case,'passed':summary['independent_passed'],'failed_checks':[c['check'] for c in checks if not c['passed']]}))
 return summary

for name in sys.argv[1:]:audit(Path(name) if '/' in name else ROOT/name)
