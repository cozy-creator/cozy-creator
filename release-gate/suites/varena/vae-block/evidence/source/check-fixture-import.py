from pathlib import Path
import hashlib
import json
import sys

sys.path[:0]=[
 '/home/fidika/cozy/.worktrees/cozy-creator/xs138-p2-vae-proof/release-gate/suites/varena/vae-block',
 '/home/fidika/cozy_v2/packages/minimax-h3-tools/src',
 '/home/fidika/cozy/.worktrees/cozy-runtime/xs138-p2-vae-block/src',
 '/home/fidika/cozy/.worktrees/packages/xs138-p2-h3-output/minimax-h3',
 '/home/fidika/cozy_v2/outputs/xs138-p2-native-retention/metadata',
 '/home/fidika/cozy_v2/outputs/xs138-native-cache-candidate-a96c7c3e8/installed',
 '/home/fidika/cozy/.worktrees/cozy-creator/xs138-p0-torch/.task/torch-sdk/2.14.0+cu130-08187d9e/site-packages',
]
import torch
from ci_varena_vae.dense import construction, PATHS
from ci_varena_vae import MODULES
from cozy_runtime.internal import dense_linear
block, linears = construction(torch.device('meta'))
assert len(linears)==6 and all(not m.weight.requires_grad for m in linears)
assert dense_linear.policy(0) is None  # No grad/autocast/CUDA qualification is invented.
assert not torch.cuda.is_initialized()
record={'passed':True,'cuda_initialized':False,'paths':PATHS,'shapes':[list(m.weight.shape) for m in linears],'runtime_modules':MODULES,'scope':'actual fixture module construction and source import only; no native module loaded or CUDA call'}
p=Path(sys.argv[1]);assert not p.exists();p.write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
