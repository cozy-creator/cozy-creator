"""Actual small H3 blocks, original eager guard/body, dual schedulers and CUDA sequence.

This is a component gate. It does not render a shortened H3 video or qualify fullmodel speed.
"""

import copy
import json
from pathlib import Path
from types import SimpleNamespace

import torch
from cozy_runtime import _build_provenance
from cozy_runtime.author import AttentionLayout, ConformanceError, attention_scope
from diffusers import MiniMaxH3Scheduler
from diffusers.modular_pipelines.minimax_h3.before_denoise import (
    MiniMaxH3SetTimestepsStep,
)

from .candidate._prepared import prepare
from .candidate.adaln_pruned import AdaLNPrunedMiniMaxH3Transformer as Candidate
from .candidate.baseline import AdaLNPrunedMiniMaxH3Transformer as Baseline

CONFIG = {
    "num_attention_heads": 2,
    "attention_head_dim": 64,
    "hidden_size": 128,
    "num_layers": 2,
    "num_refiner_layers": 1,
    "ffn_dim": 256,
    "in_channels": 4,
    "audio_in_channels": 8,
    "patch_size": (1, 2, 2),
    "text_dim": 32,
    "freq_dim": 32,
    "time_embed_hidden_dim": 128,
    "time_embed_dim": 64,
    "rope_freq_dim": 8,
}


def exercise(seed, ctx):
    device = torch.device(str(ctx.device))
    torch.set_num_threads(1)
    torch.manual_seed(seed)
    video_schedule, audio_schedule = (
        MiniMaxH3Scheduler(shift=12),
        MiniMaxH3Scheduler(shift=3),
    )
    for scheduler in (video_schedule, audio_schedule):
        scheduler.set_timesteps(31, device=device)
    text_indices = torch.arange(3, device=device)
    audio_indices = torch.arange(3, 7, device=device)
    video_indices = torch.arange(7, 13, device=device)
    tags = torch.cat((torch.ones(3), torch.full((4,), 2), torch.zeros(6))).to(
        device=device, dtype=torch.int64
    )
    positions = torch.zeros((13, 3), device=device, dtype=torch.int64)
    positions[:, 0] = torch.arange(13, device=device)
    row_plan = [
        tuple(
            value.to(device)
            for value in MiniMaxH3SetTimestepsStep.build_row_timesteps(
                video_indices.cpu(),
                audio_indices.cpu(),
                1,
                1,
                3,
                float(t),
                float(a),
                max(float(t), 0.999),
                1.0,
            )
        )
        for t, a in zip(video_schedule.timesteps, audio_schedule.timesteps, strict=True)
    ]
    times = sorted({float(value) for row, _ in row_plan for value in row.cpu()})
    kwargs = {
        **CONFIG,
        "table_timesteps": times,
        "table_block_keys": [(i, tag) for i in range(len(times)) for tag in range(3)],
    }
    candidate = Candidate(**kwargs).eval()
    with torch.no_grad():
        for parameter in candidate.parameters():
            parameter.uniform_(-0.05, 0.05)
    candidate.to(device=device, dtype=torch.bfloat16)
    candidate.time_proj.timesteps = torch.tensor(
        times, device=device, dtype=torch.float32
    )
    for name in ("proj_in", "audio_proj_in", "proj_out", "audio_proj_out"):
        getattr(candidate, name).float()
    baseline = Baseline(**kwargs).eval().to(device=device, dtype=torch.bfloat16)
    baseline.time_proj.timesteps = candidate.time_proj.timesteps.clone()
    for name in ("proj_in", "audio_proj_in", "proj_out", "audio_proj_out"):
        getattr(baseline, name).float()
    baseline.load_state_dict(candidate.state_dict(), strict=True)
    video = torch.randn(6, 16, device=device, dtype=torch.float32)
    audio = torch.randn(4, 8, device=device, dtype=torch.float32)
    prompt = torch.randn(1, 3, 32, device=device, dtype=torch.bfloat16)
    fields = {
        "token_tags": tags,
        "position_ids": positions,
        "video_indices": video_indices,
        "audio_indices": audio_indices,
        "text_indices": text_indices,
    }
    state = SimpleNamespace(
        latents=video.clone(),
        audio_latents=audio.clone(),
        prompt_embeds=prompt,
        row_timestep_plan=row_plan,
        denoiser_input_fields=fields,
        attention_kwargs={},
        num_condition_video_rows=1,
        num_condition_audio_rows=1,
    )

    def layout(index):
        return AttentionLayout(
            live_tokens=13,
            protected_prefix=8,
            step=index,
            dense_until_step=10,
            dense_paths=("token_refiner",),
        )

    ref_video, ref_audio = video.clone(), audio.clone()
    vs, aus = copy.deepcopy(video_schedule), copy.deepcopy(audio_schedule)
    rows = []
    with torch.no_grad():
        for index, (timestep, timestep_indices) in enumerate(row_plan):
            ctx.raise_if_cancelled()
            arguments = dict(
                hidden_states=ref_video[None],
                audio_hidden_states=ref_audio[None],
                encoder_hidden_states=prompt,
                timestep=timestep,
                timestep_indices=timestep_indices,
                attention_kwargs={},
                return_dict=False,
                **fields,
            )
            with attention_scope(layout(index)):
                expected = baseline(**arguments)
                actual = candidate(**arguments)
            for left, right in zip(actual, expected, strict=True):
                torch.testing.assert_close(left, right, rtol=0, atol=0)
            ref_video[1:] = vs.step(
                expected[0][0, 1:].float(),
                vs.timesteps[index],
                ref_video[1:],
                return_dict=False,
            )[0]
            ref_audio[1:] = aus.step(
                expected[1][0, 1:].float(),
                aus.timesteps[index],
                ref_audio[1:],
                return_dict=False,
            )[0]
            rows.append(
                {
                    "step": index,
                    "unique_timestep_rows": timestep.numel(),
                    "eager_factor_exact": True,
                }
            )
        # An actual invalid checkpoint row must still refuse both ordinary implementations.
        invalid = dict(
            arguments, timestep=torch.full_like(arguments["timestep"], 0.1234567)
        )
        refused = []
        for model in (baseline, candidate):
            try:
                model(**invalid)
            except ConformanceError as error:
                assert error.code == "artifact_config"
                refused.append(True)
            else:
                raise AssertionError("unknown timestep was accepted")
        plan, parts = prepare(
            candidate, state, video_schedule, audio_schedule, layout, 10
        )
        try:
            execution = ctx.plan(plan)
            actual_video, actual_audio = execution.wait()
            progress = execution.poll()
            assert (
                progress.done
                and not progress.canceled
                and progress.completed_steps == 30
            )
            torch.testing.assert_close(actual_video, ref_video, rtol=0, atol=0)
            torch.testing.assert_close(actual_audio, ref_audio, rtol=0, atol=0)
            torch.testing.assert_close(actual_video[:1], video[:1], rtol=0, atol=0)
            torch.testing.assert_close(actual_audio[:1], audio[:1], rtol=0, atol=0)
            assert sum(p.end - p.first for p in parts if p.dense) == 10
            assert sum(p.end - p.first for p in parts if not p.dense) == 20
            facts = plan.facts()
            assert facts["native_stage_advancement"] and facts[
                "host_graph_launches"
            ] == len(parts)
            return {
                "runtime_source": _build_provenance.COMMIT,
                "device": str(device),
                "source": json.loads(
                    Path(__file__).parents[1].joinpath("SOURCE.json").read_text()
                ),
                "original_vs_factored_eager": rows,
                "unknown_row_refusals": refused,
                "native_video_exact": True,
                "native_audio_exact": True,
                "conditioning_prefixes_unchanged": True,
                "completed_steps": 30,
                "segments": [
                    {
                        "first": p.first,
                        "end": p.end,
                        "dense": p.dense,
                        "unique_rows": p.unique_rows,
                    }
                    for p in parts
                ],
                "native": facts,
                "scope": "Real tinyH3 CUDA component/dualEuler/sequence parity; no fullvideo, SOLbackend or performance qualification.",
            }
        finally:
            plan.close()
