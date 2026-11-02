"""Actual file, resolver, Backend, ModelRegistry and Placement for cache lifecycle proof."""
from __future__ import annotations

from pathlib import Path

import tensorfs
import torch
from cozy_runtime.author import Loader, Model, uses_components
from cozy_runtime.author._loader import Artifact, Config
from cozy_runtime.author._model import ModelRegistry
from cozy_runtime.internal.encoding.selection import measure_device, measure_runtime
from cozy_runtime.internal.fill import Checkpoint, dtype_name, tensor_schema_of
from cozy_runtime.internal.resolution import ConstructionFacts, Variant, resolve
from cozy_runtime.internal.weights import Backend, Placement, launch_providers


class Pipeline:
    def __init__(self):
        self.body = torch.nn.Linear(4, 4, device='meta').eval()

    @property
    def components(self):
        return {'body': self.body}


def pipeline(_config: Config) -> Pipeline:
    return Pipeline()


class DerivedOwner(Model):
    def load(self, loader: Loader) -> None:
        self.pipeline = loader.construct(Pipeline, factory=pipeline)
        self.derived = loader.cache('derived_weights', max_entries=2, max_bytes=16 << 20)

    def _build(self):
        # Real immutable checkpoint weights produce the cache, not a stand-in inference result.
        return self.pipeline.body.weight.flatten().repeat(65536)

    @uses_components('body')
    def warm(self):
        return self.pipeline.body(torch.ones((1, 4), device=self.pipeline.body.weight.device))

    @uses_components('body')
    def inner(self):
        return self.derived.get('weights', build=self._build)

    @uses_components('body')
    def outer(self, inspect_owner):
        value = self.inner()  # first Region creation happens only after both scopes opened
        observation = inspect_owner(self.derived)
        again = self.derived.get('weights', build=self._build)
        output = self.pipeline.body(again[:4].view(1, 4))
        return value, output, observation, value is again


class Loaded:
    def __init__(self, memory, directory: Path):
        writer = tensorfs.Writer(directory / 'derived.cozytensors')
        component = writer.component('body')
        component.put('weight', torch.full((4, 4), .25))
        component.put('bias', torch.ones(4))
        self.source = writer.commit()
        checkpoint = Checkpoint(self.source)
        rows = checkpoint.rows('body')
        release = 'cache-consumers-fixture'
        facts = measure_device(torch, memory.device.index or 0, memory.device.type)
        plan = resolve(
            variant=Variant('actual-file', 'local', checkpoint.id), rows=rows,
            providers=launch_providers(), device=facts, runtime=measure_runtime(torch, release),
            construction=ConstructionFacts(release=release,
                model_class=f'{DerivedOwner.__module__}:{DerivedOwner.__qualname__}',
                components=('body',), custody='canonical'), dtype_name=dtype_name,
        )
        self.backend = Backend({'body': checkpoint}, rows, torch=torch, plan=plan, memory=memory,
                               device=memory.device, construction=plan.digest(), device_facts=facts,
                               release=release)
        self.backend.expect({'body': tuple(row.key for row in rows)})
        self.registry = ModelRegistry(release=release, backend=self.backend)
        self.model = self.registry.acquire('proof.models.model', DerivedOwner,
            Artifact(checkpoint.id, tensor_schema_of(rows), Config({})))
        self.placement = Placement(memory, self.backend)
        # This is the same real Placement handoff performed by executor construction.
        object.__setattr__(self.model, '_cozy_residency', self.placement)
        self.plan = plan

    def close(self):
        self.registry.unload_all()
        self.placement.release()
        self.backend.close()
        self.source.close()
