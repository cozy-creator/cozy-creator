"""Real CUDA tinyH3 component; fullmodel requests remain15seconds/30steps."""

import json

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 2697


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    from .exercise import exercise

    return Report(True, json.dumps(exercise(payload.seed, ctx), sort_keys=True))


@app.entrypoint(accelerator=False)
def inspect_source(payload: Probe) -> Report:
    import hashlib
    from pathlib import Path

    import torch

    from .candidate.adaln_pruned import AdaLNPrunedMiniMaxH3Transformer as Candidate
    from .candidate.baseline import AdaLNPrunedMiniMaxH3Transformer as Baseline
    from .exercise import CONFIG

    source = json.loads(Path(__file__).with_name("SOURCE.json").read_text())
    for name, expected in source["files"].items():
        assert (
            hashlib.sha256(
                Path(__file__).with_name("candidate").joinpath(name).read_bytes()
            ).hexdigest()
            == expected
        )
    torch.set_num_threads(1)
    torch.manual_seed(payload.seed)
    parameters = {
        **CONFIG,
        "table_timesteps": (0.0, 0.5, 1.0),
        "table_block_keys": [(i, tag) for i in range(3) for tag in range(3)],
    }
    candidate, baseline = Candidate(**parameters).eval(), Baseline(**parameters).eval()
    with torch.no_grad():
        for weight in candidate.parameters():
            weight.uniform_(-0.05, 0.05)
        baseline.load_state_dict(candidate.state_dict(), strict=True)
        arguments = {
            "hidden_states": torch.randn(1, 6, 16),
            "audio_hidden_states": torch.randn(1, 4, 8),
            "encoder_hidden_states": torch.randn(1, 3, 32),
            "timestep": torch.tensor([0.5]),
            "timestep_indices": torch.zeros(13, dtype=torch.int64),
            "token_tags": torch.tensor([1] * 3 + [2] * 4 + [0] * 6),
            "position_ids": torch.zeros((13, 3), dtype=torch.int64),
            "video_indices": torch.arange(7, 13),
            "audio_indices": torch.arange(3, 7),
            "text_indices": torch.arange(3),
            "attention_kwargs": {},
            "return_dict": False,
        }
        for left, right in zip(
            candidate(**arguments), baseline(**arguments), strict=True
        ):
            torch.testing.assert_close(left, right, rtol=0, atol=0)
    assert not torch.cuda.is_initialized()
    return Report(
        True,
        json.dumps(
            {
                "source": source,
                "cpu_factored_forward_exact": True,
                "cuda_initialized": False,
                "scope": "Actual installed fixture sourcehashes/imports and one CPUH3tensorforward; noGPU/native/modelproof.",
            },
            sort_keys=True,
        ),
    )
