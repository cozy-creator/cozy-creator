"""A small real Qwen checkpoint through ordinary Runtime construction and placement."""
from __future__ import annotations

from pathlib import Path

import tensorfs
import torch
from cozy_runtime import lm
from cozy_runtime.author import Loader, Model, uses_components
from cozy_runtime.author._loader import Artifact, Config
from cozy_runtime.author._model import ModelRegistry
from cozy_runtime.internal.encoding.selection import measure_device, measure_runtime
from cozy_runtime.internal.fill import Checkpoint, dtype_name, tensor_schema_of
from cozy_runtime.internal.resolution import ConstructionFacts, Variant, resolve
from cozy_runtime.internal.weights import Backend, Placement, launch_providers
from transformers import Qwen3Config, Qwen3ForCausalLM


def configuration():
    config = Qwen3Config(vocab_size=64, hidden_size=64, intermediate_size=128,
                        num_hidden_layers=1, num_attention_heads=4, num_key_value_heads=2,
                        head_dim=16, max_position_embeddings=256, attention_dropout=0.,
                        tie_word_embeddings=False, bos_token_id=1, eos_token_id=None, pad_token_id=0)
    config._attn_implementation = 'sdpa'
    return config


class Pipeline:
    def __init__(self):
        # A genuine small model. Runtime's Backend installs its real checkpoint bytes;
        # nonpersistent rotary values are constructed normally and placed by that backend.
        self.body = Qwen3ForCausalLM(configuration()).eval()

    @property
    def components(self):
        return {'body': self.body}


def pipeline(_config: Config):
    return Pipeline()


class LanguageModel(Model):
    def load(self, loader: Loader) -> None:
        self.pipeline = loader.construct(Pipeline, factory=pipeline)

    @uses_components('body')
    def decode(self, ids, progress=None):
        return lm.generate(self.pipeline.body, {'input_ids': ids}, max_new_tokens=8,
                           stop=[], pad=0, temperature=0., progress=progress)


class Loaded:
    def __init__(self, memory, directory: Path):
        with torch.random.fork_rng(devices=[]):
            torch.manual_seed(13871)
            original = Qwen3ForCausalLM(configuration()).eval()
        self.reference = original
        writer = tensorfs.Writer(directory / 'qwen.cozytensors')
        component = writer.component('body')
        for name, value in original.state_dict().items():
            component.put(name, value.detach())
        self.source = writer.commit()
        checkpoint = Checkpoint(self.source)
        rows = checkpoint.rows('body')
        release = 'lm-owner-fixture'
        facts = measure_device(torch, memory.device.index or 0, memory.device.type)
        plan = resolve(variant=Variant('actual-qwen-file', 'local', checkpoint.id), rows=rows,
                       providers=launch_providers(), device=facts, runtime=measure_runtime(torch, release),
                       construction=ConstructionFacts(release=release,
                           model_class=f'{LanguageModel.__module__}:{LanguageModel.__qualname__}',
                           components=('body',), custody='canonical'), dtype_name=dtype_name)
        self.backend = Backend({'body': checkpoint}, rows, torch=torch, plan=plan, memory=memory,
                               device=memory.device, construction=plan.digest(), device_facts=facts,
                               release=release)
        self.backend.expect({'body': tuple(row.key for row in rows)})
        self.registry = ModelRegistry(release=release, backend=self.backend)
        self.model = self.registry.acquire('proof.models.model', LanguageModel,
                                           Artifact(checkpoint.id, tensor_schema_of(rows), Config({})))
        self.placement = Placement(memory, self.backend)
        object.__setattr__(self.model, '_cozy_residency', self.placement)
        self.plan = plan

    def expected(self, ids):
        self.reference.to(ids.device)
        with torch.inference_mode():
            value = self.reference.generate(input_ids=ids, max_new_tokens=8,
                                            do_sample=False, eos_token_id=[], pad_token_id=0)
        result = value[:, ids.shape[1]:].cpu()
        self.reference.to('cpu')
        return result

    def close(self):
        self.registry.unload_all()
        self.placement.release()
        self.backend.close()
        self.source.close()
