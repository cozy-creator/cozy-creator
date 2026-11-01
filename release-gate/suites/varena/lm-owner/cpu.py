"""Real placed Qwen generation and idle owner retirement; CPU scope only."""
import hashlib
import importlib.metadata
import json
import sys
from pathlib import Path

import torch
from ci_varena_lm_owner.model import Loaded
from cozy_runtime import lm
from cozy_runtime.author._model import PLACED
from cozy_runtime.internal.residency import Memory

root = Path(sys.argv[1]); root.mkdir(parents=True, exist_ok=False)
assert not torch.cuda.is_initialized()
loaded = Loaded(Memory('cpu'), root)
try:
    ids = torch.tensor([[1, 4, 9]], dtype=torch.long)
    expected = loaded.expected(ids)
    body = loaded.model.pipeline.body
    active = {}

    def progress(tokens):
        if not active:
            running = lm._ENGINES[body]
            active.update(tokens=tokens, occupied=running._occupied(), pins=running.region.pins,
                          discarded=running.region.discard())

    actual = loaded.model.decode(ids, progress=progress)
    engine = lm._ENGINES[body]
    assert PLACED[body][0] is loaded.placement and engine.region is not None
    assert torch.equal(actual, expected), (actual, expected)
    assert active['occupied'] > 0 and active['pins'] > 0 and not active['discarded'], active
    assert engine._occupied() == 0 and engine.driver is None and engine.cache is not None
    old = tuple(engine.addresses.values())
    before = {'bindings': len(old), 'retained': [sum(b.retained for b in binding.groups) for binding in old],
              'cache_capacity': engine.capacity, 'region_pins': engine.region.pins,
              'tokens': actual.tolist(), 'reference': expected.tolist()}
    assert old and all(value > 0 for value in before['retained'])
    discarded = engine.region.discard()
    after = {'discarded': discarded, 'bindings': len(engine.addresses),
             'old_bindings_closed': [binding.closed for binding in old],
             'retained': [sum(b.retained for b in binding.groups) for binding in old],
             'cache_gone': engine.cache is None}
    if '--require-closed' in sys.argv[2:]:
        assert after['bindings'] == 0 and all(after['old_bindings_closed'])
        assert all(value == 0 for value in after['retained'])
    rebuilt = loaded.model.decode(ids)
    assert torch.equal(rebuilt, expected) and torch.equal(actual, expected)
    lm.release(body)
    result = {'active': active, 'before': before, 'after': after, 'rebuilt': rebuilt.tolist(),
              'public_output_preserved': torch.equal(actual, expected),
              'engine_removed_after_close': body not in lm._ENGINES,
              'real_checkpoint': loaded.source.id, 'real_model': type(body).__name__,
              'runtime_lm_sha256': hashlib.sha256(Path(lm.__file__).read_bytes()).hexdigest(),
              'torch': torch.__version__, 'transformers': importlib.metadata.version('transformers'),
              'cuda_initialized': torch.cuda.is_initialized(), 'gpu_qualified': False}
finally:
    loaded.close()
print(json.dumps(result, sort_keys=True))
