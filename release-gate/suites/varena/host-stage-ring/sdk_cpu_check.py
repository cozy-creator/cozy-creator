"""Real CPU Offload/profile integration on an explicitly sealed SDK source cohort."""
import argparse
import copy
import gc
import json
import pathlib
import sys
import weakref

import torch
from torch import nn
from tensorfs.varena import torch as vt

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from host_stage_ring import Ring
from host_stage_seal import configure


class ThreeBlocks(nn.Module):
    _no_split_modules = ['Linear']

    def __init__(self):
        super().__init__()
        self.up_blocks = nn.ModuleList([nn.Linear(3, 3) for _ in range(3)])

    def forward(self, value):
        for block in self.up_blocks:
            value = block(value)
        return value



class CPUApplication:
    def __init__(self):
        self.unet = ThreeBlocks().eval()
        self.components = {'unet': self.unet}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cohort', choices=('9d', '627'), required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    assert not torch.cuda.is_initialized()
    ring = Ring(proof=True)
    configure(ring, args.cohort)
    ring.install()
    try:
        application = CPUApplication()
        model = application.unet
        reference = copy.deepcopy(model)
        owner = vt.offload(application, device='cpu', graphs='auto')
        value = torch.ones(2, 3)
        with torch.no_grad():
            first = model(value)
            second = model(value)
            third = model(value)
            expected = reference(value)
        assert all(torch.equal(output, expected) for output in (first, second, third))
        owner_ref, module_ref = weakref.ref(owner), weakref.ref(model)
        owner.close()
        del owner, model, application, reference, value, first, second, third, expected
        gc.collect()
        assert owner_ref() is None and module_ref() is None
    finally:
        ring.stop()
    assert ring.available and ring.size > 0 and not ring.calls
    assert not torch.cuda.is_initialized()
    assert ring.window_done
    ring.metadata['host_control_only'] = True
    assert ring.write(args.output / 'ring.json')
    from analyze import analyze
    analysis, code = analyze(args.output / 'ring.json')
    assert code == 0 and analysis['host_control_only']
    rows = analysis['rows']
    assert rows[0]['boundary_name'] == 'unet.up_blocks.0' and rows[0]['dispatch_attempt'] == 2
    assert any(row['boundary_name'] == 'unet.up_blocks.1' and row['dispatch_attempt'] == 3
               and row['event_name'] == 'component_exit_exit' for row in rows)
    assert rows[-1]['boundary_name'] == 'unet.up_blocks.2' and rows[-1]['dispatch_attempt'] == 3
    (args.output / 'analysis.json').write_text(json.dumps(analysis, indent=2) + '\n')
    result = {'passed': True, 'python': sys.version, 'torch_version': torch.__version__,
              'torch_git': torch.version.git_version, 'metadata': ring.metadata, 'rows': ring.size,
              'boundaries': ring.names, 'owner_and_module_collected_with_ring_alive': True,
              'actual_cpu_outputs_equal': True, 'late_host_window_and_following_component_cleanup': True, 'cuda_initialized': False,
              'native_gpu_callback_capture_replay_qualification': False}
    (args.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
