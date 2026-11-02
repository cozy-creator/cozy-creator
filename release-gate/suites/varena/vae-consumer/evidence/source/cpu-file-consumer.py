from pathlib import Path
import hashlib
import json
import sys

sys.path[:0]=[
 '/home/fidika/cozy/.worktrees/cozy-creator/xs138-p2-vae-consumer-proof/release-gate/suites/varena/vae-consumer',
 '/home/fidika/cozy_v2/packages/minimax-h3-tools/src',
 '/home/fidika/cozy/.worktrees/cozy-runtime/xs138-p2-vae-composite/src',
 '/home/fidika/cozy/.worktrees/packages/xs138-p2-h3-vae/minimax-h3',
 '/home/fidika/cozy_v2/outputs/xs138-p2-native-retention/metadata',
 '/home/fidika/cozy_v2/outputs/xs138-native-cache-candidate-a96c7c3e8/installed',
 '/home/fidika/cozy/.worktrees/cozy-creator/xs138-p0-torch/.task/torch-sdk/2.14.0+cu130-08187d9e/site-packages',
]
import torch
from cozy_runtime.internal.residency import Memory
from ci_varena_vae_consumer.model import Loaded, make, rotary as make_rotary
from minimax_h3.vae_tiles import VAE_BLOCK

torch.set_num_threads(1)
assert not torch.cuda.is_initialized()
out=Path(sys.argv[1]);assert not out.exists();out.mkdir(parents=True)
memory=Memory.open(torch.device('cpu'))
with torch.no_grad(), memory.request('real-vae-file-cpu',plan=[]) as request:
 loaded=Loaded(request,out,torch.float32,None)
 assert request.moved==0
 slots=[(name,tuple(t.shape),tuple(t.stride()),str(t.dtype),str(t.device)) for name,t in loaded.block.named_parameters()]
 assert request.moved==0
 before_current=sorted(request.current)
 x=torch.randn(1,3,2048,generator=torch.Generator().manual_seed(13849))*.1
 rotary=make_rotary(torch.device('cpu'),3)
 reference=loaded.expected(x,rotary,1)
 actual=loaded.run(x,rotary)
 torch.testing.assert_close(actual,reference,rtol=0,atol=0)
 moved=request.moved
 assert moved>0
 source_bytes=loaded.payload_bytes
 hooks=len(loaded.model.source_hooks)
 loaded.close();assert not memory.models
meta=make('meta',torch.float16)
assert all(m.bias is None or m.bias.dtype is torch.float32 for m in meta.modules() if isinstance(m,torch.nn.Linear))
assert not torch.cuda.is_initialized()
record={'passed':True,'cuda_initialized':False,'real_cpu_checkpoint_bytes':source_bytes,'source_bytes_read':moved,'metadata_inventory_read_bytes':0,'ordinary_source_hooks':hooks,'request_current_before_source_invocation':before_current,'actual_package_run_bit_exact':True,'output_shape':list(actual.shape),'mixed_matrix_bias_contract':True,'kernel_identity':VAE_BLOCK.kernels.identity,'slots':slots,'scope':'actual CPU TensorFS file, metadata-only registration, source hooks, package run and whole output; no native CUDA/fused allocation proof'}
(out/'result.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps({k:v for k,v in record.items() if k!='slots'}))
