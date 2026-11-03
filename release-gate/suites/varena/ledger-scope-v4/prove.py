"""Real GPU-hidden runc namespace/ledger proof, using exact-module v3/v4 CPU peers."""
from __future__ import annotations
import argparse, hashlib, json, os, selectors, shutil, subprocess, tempfile, time
from pathlib import Path

parser=argparse.ArgumentParser()
parser.add_argument('--peers',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True)
parser.add_argument('--image',default='sha256:4131ddd7acb9c6c966af02fe7e71edc51e078c2d41f475966697676430bfa2ae')
parser.add_argument('--device-key',required=True,help='caller-supplied metadata device key; this test discovers no GPU')
args=parser.parse_args();args.peers=args.peers.resolve();args.output=args.output.resolve();args.output.mkdir(parents=True,exist_ok=False)
root=Path(tempfile.mkdtemp(prefix='xs138-ledger-scope-v4-',dir='/dev/shm'))
actors=[];report={'passed':False,'gpu_execution':False,'metadata_only':True,'device_key':args.device_key,
                 'gpu_identity_discovered':False,'private_root':str(root),'image':args.image,'cases':{},'actors':[]}
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
report['binaries']={name:sha(args.peers/('peer-'+name)) for name in ('v3','v4')}
PREFIX=r'''
import json,os,sys
from pathlib import Path
scope={'uid':os.geteuid(),'boot':Path('/proc/sys/kernel/random/boot_id').read_text().strip(),**{k+'_namespace':os.stat('/proc/self/ns/'+n).st_ino for k,n in [('user','user'),('pid','pid'),('mount','mnt')]}}
libs=[line.split()[-1] for line in Path('/proc/self/maps').read_text().splitlines() if 'libcuda.so' in line or 'libnvidia-ml' in line]
assert not libs
print(json.dumps({'operation':'preflight','pid':os.getpid(),'start_ticks':int(Path('/proc/self/stat').read_text().rsplit(') ',1)[1].split()[19]),'scope':scope,'gpu_libraries':libs}),flush=True)
os.execv(sys.argv[1],sys.argv[1:])
'''
def command(values):return subprocess.run(values,capture_output=True,text=True)
def save():
    report['actors']=[a.record for a in actors]
    (args.output/'report.json').write_text(json.dumps(report,indent=2)+'\n')
def identity(path):
    s=path.stat();return {'dev':s.st_dev,'ino':s.st_ino,'size':s.st_size,'sha256':sha(path)}
def directory(name):
    p=root/name;p.mkdir(mode=0o700);return p
class Actor:
    def __init__(self,label,version,directory,*,mode='hold',pid_host=True,uid=1000,target='/ledger'):
        self.label=label;self.lines=[];self.process=None;self.buffer=b''
        self.record={'label':label,'version':version,'directory':str(directory),'pid_host':pid_host,'uid':uid,'target':target}
        actors.append(self)
        cmd=['docker','create','--interactive','--pull=never','--name',f'xs138-ledger-{os.getpid()}-{label}',
             '--runtime=runc','--network=none','--cpus=0.25','--user',f'{uid}:{uid}','--userns=host',
             '--cap-drop=ALL','--security-opt=no-new-privileges','-e','NVIDIA_VISIBLE_DEVICES=void','-e','CUDA_VISIBLE_DEVICES=',
             '--mount',f'type=bind,src={args.peers},dst=/proof,readonly','--mount',f'type=bind,src={directory},dst={target}',
             '--entrypoint','/opt/python/cp312-cp312/bin/python']
        if pid_host:cmd+=['--pid=host']
        cmd += [args.image,'-I','-c',PREFIX,'/proof/peer-'+version,mode,target,args.device_key]
        self.record['create_command']=cmd;r=command(cmd)
        assert r.returncode==0,r.stderr
        self.cid=r.stdout.strip();self.record['id']=self.cid
        self.stderr=(args.output/(label+'.stderr')).open('w')
        self.process=subprocess.Popen(['docker','start','--attach','--interactive',self.cid],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.stderr,bufsize=0)
        self.preflight=self.read();assert self.preflight['operation']=='preflight' and not self.preflight['gpu_libraries']
        self.ready=self.read();self.record.update(preflight=self.preflight,ready=self.ready);save()
    def read(self):
        deadline=time.monotonic()+20
        while b'\n' not in self.buffer:
            with selectors.DefaultSelector() as selected:
                selected.register(self.process.stdout,selectors.EVENT_READ)
                assert selected.select(max(0,deadline-time.monotonic())),f'{self.label}: explicit peer response deadline'
                data=os.read(self.process.stdout.fileno(),65536)
            assert data,f'{self.label}: empty response, process={self.process.poll()}'
            self.buffer+=data
        line,self.buffer=self.buffer.split(b'\n',1)
        self.lines.append(json.loads(line));(args.output/(self.label+'.jsonl')).write_text('\n'.join(json.dumps(x) for x in self.lines)+'\n')
        return self.lines[-1]
    def call(self,text):
        self.process.stdin.write((text+'\n').encode());self.process.stdin.flush();return self.read()
    def remove(self):
        if not getattr(self,'cid',None):return
        r=command(['docker','rm','--force',self.cid]);deadline=time.monotonic()+10
        while True:
            absent=command(['docker','inspect',self.cid])
            if absent.returncode!=0 or time.monotonic()>=deadline:break
            time.sleep(.05)
        exact_alive=False
        if hasattr(self,'preflight'):
            stat=Path('/proc')/str(self.preflight['pid'])/'stat'
            try:exact_alive=int(stat.read_text().rsplit(') ',1)[1].split()[19])==self.preflight['start_ticks']
            except FileNotFoundError:pass
        self.record['cleanup']={'remove_status':r.returncode,'remove_stderr':r.stderr.strip(),'absent':absent.returncode!=0 and 'no such object' in absent.stderr.lower(),'same_process_alive':exact_alive}
        if self.process is not None:
            try:self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:self.record['docker_follower_still_running']=True
        self.stderr.close()
        assert self.record['cleanup']['absent'] and not exact_alive,self.record['cleanup']

def compatible(a,b):
    x,y=a.preflight['scope'],b.preflight['scope']
    assert x['mount_namespace']!=y['mount_namespace']
    assert all(x[k]==y[k] for k in ('uid','boot','user_namespace','pid_namespace'))
def successful(actor,version):
    assert actor.ready['ok'],actor.ready
    s=actor.ready['snapshot'];assert s['protocol_version']==version and s['header'][1]==version
    assert not actor.ready['gpu_libraries']
    assert s['scope']['mount_namespace']==actor.preflight['scope']['mount_namespace']
    if version==4:assert (s['header'][12],s['header'][14])==(s['backing_device'],s['backing_inode'])
    return s

def refused(actor):
    assert actor.ready['ok'] is False and 'incompatible ledger' in actor.ready['error'],actor.ready

try:
    assert command(['docker','image','inspect',args.image]).returncode==0,'cached pinned image required'
    old=directory('old-v3');a=Actor('old-a','v3',old,target='/view-a');sa=successful(a,3)
    path=old/Path(sa['path']).name;before=identity(path)
    b=Actor('old-b','v3',old,mode='inspect',target='/view-b');compatible(a,b);refused(b)
    assert identity(path)==before
    report['cases']['actual_v3_different_mount_refusal']={'a':a.preflight,'b':b.preflight,'refusal':b.ready,'file_unchanged':before}
    a.remove();b.remove();save()

    shared=directory('shared-v4');a=Actor('shared-a','v4',shared,target='/first');sa=successful(a,4)
    b=Actor('shared-b','v4',shared,target='/second');sb=successful(b,4);compatible(a,b)
    assert (sa['backing_device'],sa['backing_inode'])==(sb['backing_device'],sb['backing_inode'])
    def rows(value):return {r['pid']:r for r in value['snapshot']['rows']}
    published_a=a.call('publish');assert published_a['ok'];seen_b=b.call('snapshot');assert seen_b['ok']
    assert rows(published_a)==rows(seen_b) and set(rows(seen_b))=={a.preflight['pid'],b.preflight['pid']}
    published_b=b.call('publish');seen_a=a.call('snapshot');assert rows(published_b)==rows(seen_a)
    assert all(r['physical']==0 for r in rows(seen_a).values())
    report['cases']['actual_v4_shared_mount_positive']={'a':a.preflight,'b':b.preflight,'a_seen_by_b':seen_b,'b_seen_by_a':seen_a}
    path=shared/Path(sa['path']).name

    private_pid=Actor('private-pid','v4',shared,mode='inspect',pid_host=False);refused(private_pid)
    assert private_pid.preflight['scope']['pid_namespace']!=a.preflight['scope']['pid_namespace']
    report['cases']['actual_private_pid_refusal']={'preflight':private_pid.preflight,'refusal':private_pid.ready};private_pid.remove()
    wrong_uid=Actor('wrong-uid','v4',shared,mode='inspect',uid=1001)
    assert not wrong_uid.ready['ok'] and wrong_uid.preflight['scope']['uid']==1001
    report['cases']['actual_uid_refusal']={'preflight':wrong_uid.preflight,'refusal':wrong_uid.ready};wrong_uid.remove()

    copied=directory('copied-v4');copy=copied/path.name;shutil.copy2(path,copy);before=identity(copy)
    copied_actor=Actor('copied','v4',copied,mode='inspect');refused(copied_actor)
    assert identity(copy)==before and before['ino']!=sa['backing_inode']
    report['cases']['copied_nonempty_backing_refusal']={'file':before,'refusal':copied_actor.ready};copied_actor.remove()

    a.remove();reaped=b.call('reap');assert reaped['ok'] and reaped['reaped']==1
    assert set(rows(reaped))=={b.preflight['pid']} and not reaped['snapshot']['sample_present']
    report['cases']['actual_ofd_death_reap']={'after_exact_a_removal':reaped,'gpu_release_claim':False}
    b.remove();save()

    left=Actor('private-a','v4',directory('private-a'));right=Actor('private-b','v4',directory('private-b'))
    sl,sr=successful(left,4),successful(right,4);compatible(left,right)
    assert (sl['backing_device'],sl['backing_inode'])!=(sr['backing_device'],sr['backing_inode'])
    left.call('publish');isolated=right.call('snapshot')
    assert set(rows(isolated))=={right.preflight['pid']}
    report['cases']['same_path_private_backing_isolated']={'left':sl,'right':isolated['snapshot']}
    left.remove();right.remove()

    for kind in ('replace','symlink'):
        folder=directory(kind);actor=Actor(kind,'v4',folder);s=successful(actor,4);original=folder/Path(s['path']).name
        retained=folder/(original.name+'.preserved');original.rename(retained);prior=identity(retained)
        if kind=='replace':shutil.copy2(retained,original)
        else:original.symlink_to(retained.name)
        answer=actor.call('snapshot');assert not answer['ok'] and 'incompatible ledger' in answer['error'],answer
        assert identity(retained)==prior
        report['cases'][kind+'_existing_connection_refusal']={'answer':answer,'old_file_preserved':prior}
        actor.remove()

    migration=Actor('mount-migration','v4',directory('mount-migration'));successful(migration,4)
    answer=migration.call('migrate_mount')
    if answer.get('qualified'):
        assert answer['before']['mount_namespace']!=answer['after']['mount_namespace'] and answer['refused'],answer
    else:
        assert answer.get('unavailable_errno') in (1,13),answer
        assert migration.call('snapshot')['ok']
    report['cases']['actual_same_connection_mount_migration']=answer;migration.remove()
    report['passed']=True
except BaseException as error:
    report['error']=repr(error)
    raise
finally:
    errors=[]
    for actor in actors:
        try:actor.remove()
        except BaseException as error:errors.append({'actor':actor.label,'error':repr(error)})
    report['cleanup_errors']=errors;report['all_owned_containers_absent']=not errors and all(a.record.get('cleanup',{}).get('absent') for a in actors)
    report['private_files_preserved']=True
    if errors:report['passed']=False
    save();print(json.dumps({'passed':report['passed'],'report':str(args.output/'report.json'),'cases':len(report['cases']),'all_owned_containers_absent':report['all_owned_containers_absent'],'gpu_execution':False}))
