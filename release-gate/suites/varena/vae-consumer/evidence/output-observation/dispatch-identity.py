from pathlib import Path
import sys,json
sys.path.insert(0,'/home/fidika/cozy/.worktrees/cozy-creator/xs138-p0-torch/.task/torch-sdk/2.14.0+cu130-08187d9e/site-packages')
import torch
from torch.utils._python_dispatch import TorchDispatchMode
class Observe(TorchDispatchMode):
 def __init__(self):
  super().__init__();self.inference=torch.is_inference_mode_enabled();self.grad=torch.is_grad_enabled();self.output=None
 def __torch_dispatch__(self,func,types,args=(),kwargs=None):
  with torch.inference_mode(self.inference),torch.set_grad_enabled(self.grad):result=func(*args,**(kwargs or {}))
  if str(func)=='aten.empty.memory_format':self.output=result
  return result
rows=[]
for inference in (False,True):
 for grad in (False,True):
  with torch.inference_mode(inference),torch.set_grad_enabled(grad):
   observed=Observe()
   with observed: output=torch.empty((3,4,8),dtype=torch.float32)
   row={'inference':inference,'grad':grad,'python_is':output is observed.output,'data_ptr_equal':output.data_ptr()==observed.output.data_ptr(),'tensor_impl_equal':output._cdata==observed.output._cdata,'storage_equal':output.untyped_storage()._cdata==observed.output.untyped_storage()._cdata,'output_inference':output.is_inference(),'observed_inference':observed.output.is_inference()}
   rows.append(row)
assert not torch.cuda.is_initialized()
result={'rows':rows,'torch_version':torch.__version__,'cuda_initialized':False}
Path(__file__).with_suffix('.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))
