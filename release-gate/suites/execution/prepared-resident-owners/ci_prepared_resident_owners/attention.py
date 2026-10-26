"""Actual SDPA capture checks, independent of the LoRA owner fixture's imports."""

import importlib.metadata

import torch
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.internal import accel


def attention_proof(seed, ctx):
    """Real zero-dropout SDPA replay; seeded dropout and random generation still refuse."""
    assert ctx.device.type == "cuda"
    accel.initialize(torch, "cuda")
    query = torch.arange(512, device="cuda", dtype=torch.float32).reshape(1, 2, 8, 32)
    query = (query / 512).to(torch.float16)
    key, value = query.clone(), query * 0.5
    with torch.inference_mode(False):
        state = torch.zeros_like(query)
    expected = torch.nn.functional.scaled_dot_product_attention(
        query, key, value, dropout_p=0.0
    )

    def step(index):
        del index
        state.copy_(
            torch.nn.functional.scaled_dot_product_attention(
                query, key, value, dropout_p=0.0
            )
        )

    plan = PreparedRepeat.capture(step, state=(state,), steps=2)
    try:
        output = ctx.plan(plan).wait()[0]
        torch.testing.assert_close(output, expected, rtol=0, atol=0)
        assert plan.facts()["host_graph_launches"] == 1
    finally:
        plan.close()

    def dropout(index):
        del index
        state.copy_(
            torch.nn.functional.scaled_dot_product_attention(
                query, key, value, dropout_p=0.25
            )
        )

    def random(index):
        del index
        state.copy_(torch.rand_like(state))

    refused = []
    saved = state.clone()
    for name, body in (("dropout", dropout), ("random", random)):
        try:
            unexpected = PreparedRepeat.capture(body, state=(state,), steps=2)
        except RuntimeError as error:
            assert "explicit noise/RNG tensor inputs" in str(error), str(error)
            refused.append(name)
        else:
            unexpected.close()
            raise AssertionError(f"seeded {name} was accepted")
        torch.testing.assert_close(state, saved, rtol=0, atol=0)
    return {
        "seed": seed,
        "mode": "attention",
        "runtime": importlib.metadata.version("cozy-runtime"),
        "tensorfs": importlib.metadata.version("tensorfs"),
        "torch": str(torch.__version__),
        "device": torch.cuda.get_device_name(0),
        "zero_dropout_host_graph_launches": 1,
        "refused": refused,
    }
