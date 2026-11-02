"""Actual CPU CLIP adoption in the stock GPU cohort's Transformers version."""

import json
from pathlib import Path

import torch
import transformers
from tensorfs.varena import torch as vt
from transformers import CLIPTextConfig, CLIPTextModel, CLIPTextModelWithProjection

torch.set_num_threads(1)
torch.set_num_interop_threads(1)
torch.manual_seed(17)
config = CLIPTextConfig(
    vocab_size=32,
    hidden_size=16,
    intermediate_size=24,
    num_hidden_layers=2,
    num_attention_heads=2,
    max_position_embeddings=8,
    projection_dim=8,
    bos_token_id=0,
    eos_token_id=3,
    pad_token_id=1,
)
rows = []
for cls in (CLIPTextModel, CLIPTextModelWithProjection):
    model = cls(config).eval()
    inputs = torch.tensor([[1, 2, 3, 4]])
    identities = [id(p) for p in model.parameters()]
    with torch.no_grad():
        expected = model(inputs, output_hidden_states=True).to_tuple()
    owner = vt.offload(model, device="cpu")
    with torch.no_grad():
        actual = model(inputs, output_hidden_states=True).to_tuple()
    torch.testing.assert_close(actual, expected, rtol=0, atol=0)
    assert [id(p) for p in model.parameters()] == identities
    facts = owner.intent_facts()
    assert facts["active"] is None and not facts["native"]
    assert any(count for count in facts["entries"].values())
    old = next(model.parameters()).detach()
    old_copy = old.clone()
    model.half()
    model.float()
    assert [id(p) for p in model.parameters()] == identities
    assert torch.equal(old.clone(), old_copy)
    with torch.no_grad():
        converted = model(inputs).to_tuple()
    rows.append(
        {
            "class": cls.__name__,
            "exact_cpu_forward": True,
            "identity_dtype_aliases": True,
            "declarations": facts,
        }
    )
    owner.close()
assert not torch.cuda.is_initialized()
assert not any(
    "libcuda.so" in line for line in Path("/proc/self/maps").read_text().splitlines()
)
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "transformers": transformers.__version__,
            "model_downloads": False,
            "gpu_executed": False,
            "rows": rows,
        }
    )
)
