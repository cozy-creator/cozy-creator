"""Actual _CaptureBindings/CUDA graph control for the eager helper; unrun GPU gate."""
from __future__ import annotations

import argparse
import json
import torch
from tensorfs.varena import torch as vt
from tensorfs.varena import _offload as adapter
from tensorfs.varena._offload_stream import component_scope

parser = argparse.ArgumentParser()
parser.add_argument('--device', type=int, default=0)
parser.add_argument('--gpu-uuid', required=True)
args = parser.parse_args()
assert hasattr(adapter.Offload, 'capture'), 'requires the actual composed graph frontend'
assert hasattr(adapter, '_GRAPH_BINDING')
vt.install(args.device)
torch.cuda.set_device(args.device)
normalize = lambda x: str(x).removeprefix('GPU-').replace('-', '').lower()
assert normalize(torch.cuda.get_device_properties(args.device).uuid) == normalize(args.gpu_uuid)
owner_box = {}
seen = []


class ScopedLinear(torch.nn.Linear):
    def forward(self, value):
        binding = adapter._GRAPH_BINDING.get()
        with component_scope(owner_box['handle']._owner, (value,), capture_bindings=binding) as scope:
            if binding is not None:
                assert scope.capture and scope.stream == binding.stream and scope.completion is None
                assert scope.checkpoint() is None
                seen.append({'actual_binding': True, 'actual_capture': torch.cuda.is_current_stream_capturing()})
            return scope.output(super().forward(value))


model = ScopedLinear(4, 4).eval()
with torch.no_grad():
    model.weight.fill_(0.125)
    model.bias.fill_(0.25)
expected = torch.full((2, 4), 0.75)
handle = vt.offload(model, device=args.device)
owner_box['handle'] = handle
stream = torch.cuda.Stream(device=args.device)
segment = None
try:
    with torch.inference_mode(), torch.cuda.stream(stream):
        value = torch.ones((2, 4), device=args.device)
        segment = handle.capture(model, (value,), stream=stream, reserve_bytes=2 << 20)
        result = segment(value, stream=stream)
        stream.synchronize()
        torch.testing.assert_close(result.cpu(), expected, rtol=0, atol=0)
        assert seen and any(row['actual_capture'] for row in seen)
        del result
finally:
    stream.synchronize()
    if segment is not None:
        segment.close()
    handle.close()
print(json.dumps({'passed': True, 'actual_capture_bindings': seen,
                  'eager_completion_absent_during_binding': True, 'actual_replay_readback': True,
                  'scope': 'helper binding bypass only; existing graph lifetime controls remain separate'}))
