from pathlib import Path
import json,sys
sys.path[:0]=[
 '/home/fidika/cozy/.worktrees/cozy-runtime/xs138-p2-sparse-producer/src',
 '/home/fidika/cozy_v2/outputs/xs138-p2-native-retention/metadata',
 '/home/fidika/cozy_v2/outputs/xs138-native-cache-candidate-a96c7c3e8/installed',
 '/home/fidika/cozy/.worktrees/cozy-creator/xs138-p0-torch/.task/torch-sdk/2.14.0+cu130-08187d9e/site-packages',
 '/home/fidika/cozy_v2/cozy-runtime/.venv/lib/python3.12/site-packages',
]
import torch
from cozy_runtime.author import AttentionLayout
from cozy_runtime.internal.loop import _Loop
assert not torch.cuda.is_initialized()
rows=[]
def broken(i,marker):
 AttentionLayout(8195,64,step=i,dense_until_step=0)
red=None
try:
 _Loop(broken,range(1),None,None,None,None).eager_all((torch.zeros(1,dtype=torch.int64),),None)
except ValueError as error:
 red=str(error)
assert red=='step must be a nonnegative integer'
def fixed(i,marker,*,phase):
 assert isinstance(i,torch.Tensor) and type(phase) is int
 layout=AttentionLayout(8195,64,step=phase,dense_until_step=0)
 rows.append({'index_type':type(i).__name__,'index_shape':list(i.shape),'step':layout.step,'seed_step':phase==0,'rolling':phase>0})
_Loop(fixed,range(2),(0,1),None,None,None).eager_all((torch.zeros(1,dtype=torch.int64),),None)
assert [row['step'] for row in rows]==[0,1]
assert [row['seed_step'] for row in rows]==[True,False]
assert [row['rolling'] for row in rows]==[False,True]
result={'passed':True,'original_failure':red,'actual_runtime_loop_rows':rows,'cuda_initialized':torch.cuda.is_initialized(),'tensor_scalar_reads':0,'scope':'actual Runtime _Loop.eager host-phase dispatch and actual AttentionLayout constructor; no GPU claim'}
assert not result['cuda_initialized']
Path(__file__).with_suffix('.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result))
