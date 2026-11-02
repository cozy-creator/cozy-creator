"""Run genuine nested component scopes and DerivedCache against the selected Runtime source."""
import json
import sys
from pathlib import Path

import torch
from ci_varena_cache_consumers.model import Loaded
from cozy_runtime.internal.residency import Memory

root = Path(sys.argv[1])
root.mkdir(parents=True, exist_ok=False)
assert not torch.cuda.is_initialized()
loaded = Loaded(Memory('cpu'), root)
try:
    with torch.no_grad():
        warm = loaded.model.warm()
        assert torch.equal(warm, torch.full((1, 4), 2.))
        assert loaded.model.derived._region is None

        def inspect(cache):
            region = cache._region
            return {'pins_after_inner': region.pins, 'discarded_during_outer': region.discard(),
                    'entries_after_attempt': cache.entries}

        builds = [0]
        code = type(loaded.model)._build.__code__
        assert sys.getprofile() is None

        def profile(frame, event, arg):
            if event == 'call' and frame.f_code is code and frame.f_locals.get('self') is loaded.model:
                builds[0] += 1

        sys.setprofile(profile)
        try:
            escaped, output, held, reused = loaded.model.outer(inspect)
        finally:
            sys.setprofile(None)
        idle = loaded.model.derived._region
        state = {'during_outer': held, 'same_entry_after_attempt': reused,
                 'builds': builds[0], 'pins_after_outer': idle.pins,
                 'output': output.tolist(), 'cache_bytes': escaped.nbytes}
        assert torch.equal(output, torch.full((1, 4), 1.25))
        assert bool((escaped == .25).all())
        state['discarded_when_idle'] = idle.discard()
        state['escaped_readback_after_discard'] = bool((escaped == .25).all())
        state['entries_after_idle_discard'] = loaded.model.derived.entries
finally:
    loaded.close()
assert not torch.cuda.is_initialized()
state.update(cuda_initialized=False, real_source=True, real_registry=True,
             real_backend=True, real_placement=True, real_component_scopes=True,
             region_closed_after_unload=idle.closed,
             cache_region_cleared_after_unload=loaded.model.derived._region is None)
print(json.dumps(state, sort_keys=True))
