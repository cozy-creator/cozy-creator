"""Production-width H3 DiT layers: eager versus native resident segments with Kitchen summaries.

Geometry is the real FL2VA 15 s request: 362 frames at 1344x768 (107 latent frames of 24x42
patches), first and last keyframe anchors, stereo audio, 30 steps. The DiT has H3's real widths
(hidden 5376, FFN 14336, 56 heads of 128, rotary 96) and fewer layers; weights are random but
stored, placed, filled and served by Runtime as a model's are, so attention selection, H3's glue
and the Kitchen producer are the production ones.
"""

from __future__ import annotations

import copy
import dataclasses
import hashlib
import importlib.metadata
import json
import sys
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import torch
from cozy_runtime import _build_provenance
from cozy_runtime.author import AttentionLayout, CapabilityError, attention_scope
from cozy_runtime.internal.producers import Processor
from diffusers import MiniMaxH3Scheduler
from diffusers.modular_pipelines.minimax_h3.before_denoise import (
    MiniMaxH3PrepareLayoutStep,
    MiniMaxH3SetTimestepsStep,
)
from diffusers.modular_pipelines.minimax_h3.modular_pipeline import (
    audio_latent_num_frames,
    video_latent_num_frames,
)

from .candidate import producer
from .candidate._prepared import prepare
from .candidate.adaln_pruned import AdaLNPrunedMiniMaxH3Transformer, step_invariants

SOURCE = json.loads(Path(__file__).with_name("source.json").read_text())
FRAMES, HEIGHT, WIDTH, STEPS = 362, 768, 1344, 30
#: H3's DiT config (Diffusers MiniMaxH3Transformer3DModel defaults); layers are the gate's.
CONFIG = {
    "num_attention_heads": 56,
    "attention_head_dim": 128,
    "hidden_size": 5376,
    "ffn_dim": 14336,
    "in_channels": 24,
    "audio_in_channels": 32,
    "patch_size": [1, 2, 2],
    "text_dim": 5120,
    "freq_dim": 256,
    "time_embed_hidden_dim": 5376,
    "time_embed_dim": 2688,
    "rope_freq_dim": 16,
}


def verify_source() -> None:
    root = Path(__file__).with_name("candidate")
    for name, digest in SOURCE["files"].items():
        assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name


def geometry(latent_frames: int, text_tokens: int) -> dict[str, Any]:
    """The packed FL2VA layout upstream builds (`build_packed_sequence`)."""
    position_ids, tags, video, audio, text, ncv, nca = MiniMaxH3PrepareLayoutStep.build_packed_sequence(
        text_token_tags=torch.ones(text_tokens, dtype=torch.long),
        num_latent_frames=latent_frames,
        latent_height=HEIGHT // 16,
        latent_width=WIDTH // 16,
        num_audio_latents=audio_latent_num_frames(FRAMES),
        patch_size=(1, 2, 2),
        audio_channels=2,
        audio_tag=2,
        video_tag=0,
        keyframe_anchors=("first", "last"),
    )
    fields = {"token_tags": tags, "position_ids": position_ids, "video_indices": video,
              "audio_indices": audio, "text_indices": text}
    return {"fields": fields, "ncv": ncv, "nca": nca}


def schedule(layout: dict[str, Any]) -> tuple[Any, Any, list[tuple[torch.Tensor, torch.Tensor]]]:
    """Both official schedulers (shift 12 / 3) and the per-step row plan upstream builds."""
    video_schedule, audio_schedule = MiniMaxH3Scheduler(shift=12.0), MiniMaxH3Scheduler(shift=3.0)
    for scheduler in (video_schedule, audio_schedule):
        # H3's sigma grid has evaluations + 1 points (official TimestepPlan.sigma_grid_points).
        scheduler.set_timesteps(STEPS + 1)
    fields = layout["fields"]
    rows = [
        MiniMaxH3SetTimestepsStep.build_row_timesteps(
            fields["video_indices"], fields["audio_indices"], layout["ncv"], layout["nca"],
            fields["text_indices"].numel(), float(t), float(a), max(float(t), 0.999), 1.0,
        )
        for t, a in zip(video_schedule.timesteps, audio_schedule.timesteps, strict=True)
    ]
    return video_schedule, audio_schedule, rows


def tables() -> tuple[list[float], list[list[int]]]:
    """Every row timestep of the schedule (geometry-independent), and every (row, modality)."""
    _, _, rows = schedule(geometry(2, 4))
    times = sorted({float(v) for row, _ in rows for v in row})
    return times, [[i, tag] for i in range(len(times)) for tag in range(3)]


def build(config: dict[str, Any]) -> AdaLNPrunedMiniMaxH3Transformer:
    """The DiT in the dtypes its checkpoint stores: official._apply_transformer_dtype's policy
    (each root child F32 if the class keeps it in F32, else BF16), for writer and loader alike."""
    model = AdaLNPrunedMiniMaxH3Transformer.from_official_config(
        config,
        table_timesteps=config["table_timesteps"],
        table_block_keys=[tuple(key) for key in config["table_block_keys"]],
    )
    for name, child in model.named_children():
        child.to(dtype=torch.float32 if name in model._keep_in_fp32_modules else torch.bfloat16)
    return model.eval()


def checkpoint(seed: int, layers: int, refiner_layers: int) -> tuple[dict[str, Any], dict[str, torch.Tensor]]:
    """Deterministic weights: PyTorch's own init, AdaLN tables kept small so activations stay
    finite through the layers, in `build`'s dtypes."""
    times, keys = tables()
    config = {**CONFIG, "num_layers": layers, "num_refiner_layers": refiner_layers,
              "table_timesteps": times, "table_block_keys": keys}
    with torch.random.fork_rng(devices=[]):
        torch.manual_seed(seed)
        model = build(config)
        with torch.no_grad():
            for name, parameter in model.named_parameters():
                if name.endswith(".table"):
                    parameter.normal_(0.0, 0.02)
    return config, {name: value.detach() for name, value in model.state_dict().items()}


# ---- the gate --------------------------------------------------------------------------------
def sites(transformer: Any) -> dict[str, str]:
    """Each attention site whose Runtime wrapper serves this package's Kitchen producer."""
    served = {}
    for path, module in transformer.named_modules():
        processor = getattr(module, "processor", None)
        if isinstance(processor, Processor):
            selected = processor._attention_backend
            if getattr(selected, "value", str(selected)) == processor.backend and (
                producer.PRODUCERS.get(processor.producer.name) is processor.producer
            ):
                served[path] = processor.producer.name
    return served


def backends(transformer: Any) -> dict[str, str]:
    return {
        path: str(getattr(module.processor, "_attention_backend", None))
        for path, module in transformer.named_modules()
        if hasattr(module, "processor")
    }


class Request:
    """One request's inputs on the device, and fresh schedulers per pass."""

    def __init__(self, seed: int, latent_frames: int, text_tokens: int, device: torch.device) -> None:
        layout = geometry(latent_frames, text_tokens)
        self.video_schedule, self.audio_schedule, rows = schedule(layout)
        self.rows = [(t.to(device), i.to(device)) for t, i in rows]
        self.fields = {k: v.to(device) for k, v in layout["fields"].items()}
        self.ncv, self.nca = layout["ncv"], layout["nca"]
        generator = torch.Generator().manual_seed(seed)
        tokens = self.fields["token_tags"].numel()
        video_rows, audio_rows = self.fields["video_indices"].numel(), self.fields["audio_indices"].numel()
        self.video = torch.randn(video_rows, 96, generator=generator).to(device)
        self.audio = torch.randn(audio_rows, 32, generator=generator).to(device)
        self.prompt = (torch.randn(1, text_tokens, 5120, generator=generator) * 0.5).to(device, torch.bfloat16)
        self.tokens = tokens

    def layout(self, step: int) -> AttentionLayout:
        # official._attention_layout, with the policy read from official.py at the candidate commit.
        return AttentionLayout(
            live_tokens=self.tokens,
            protected_prefix=self.fields["text_indices"].numel() + self.ncv + self.fields["audio_indices"].numel(),
            step=step,
            dense_until_step=SOURCE["sol_dense_steps"],
            dense_paths=tuple(SOURCE["dense_paths"]),
            exact_paths=tuple(SOURCE["exact_paths"]),
        )

    def state(self) -> SimpleNamespace:
        return SimpleNamespace(
            latents=self.video.clone(), audio_latents=self.audio.clone(), prompt_embeds=self.prompt,
            row_timestep_plan=self.rows, denoiser_input_fields=self.fields, attention_kwargs={},
            num_condition_video_rows=self.ncv, num_condition_audio_rows=self.nca,
        )


def statistics(name: str) -> dict[str, Any]:
    held = producer.PRODUCERS[name].request_state()
    return {} if held is None else held.statistics


def eager(transformer, request, state, schedulers, first, end, name, trace=None, tick=None):
    """Steps [first, end) exactly as OfficialH3Pipeline.denoise runs them."""
    vs, aus = schedulers
    for index in range(first, end):
        times, indices = request.rows[index]
        began = time.perf_counter()
        with torch.no_grad(), attention_scope(request.layout(index)):
            video, audio = transformer(
                hidden_states=state.latents[None], audio_hidden_states=state.audio_latents[None],
                encoder_hidden_states=request.prompt, timestep=times, timestep_indices=indices,
                attention_kwargs={}, return_dict=False, **request.fields,
            )
            state.latents[request.ncv:] = vs.step(
                video[0, request.ncv:].float(), vs.timesteps[index], state.latents[request.ncv:],
                return_dict=False)[0]
            state.audio_latents[request.nca:] = aus.step(
                audio[0, request.nca:].float(), aus.timesteps[index], state.audio_latents[request.nca:],
                return_dict=False)[0]
        torch.cuda.synchronize()
        if tick is not None:
            tick(index)
        if trace is not None:
            stats = {key[1]: (m.cpu(), s.cpu()) for key, (step, _, m, s) in statistics(name).items() if step == index}
            trace.append({"step": index, "ms": (time.perf_counter() - began) * 1000, "stats": stats,
                          "video": digest(state.latents), "audio": digest(state.audio_latents),
                          "finite": bool(state.latents.isfinite().all() and state.audio_latents.isfinite().all())})


def digest(tensor: torch.Tensor) -> str:
    return hashlib.sha256(tensor.detach().cpu().contiguous().view(torch.uint8).numpy().tobytes()).hexdigest()[:16]


def native(transformer, request, state, schedulers, start, ctx):
    """prepare() + one native submission, as _prepared.run does; the carry's final summaries."""
    vs, aus = schedulers
    began = time.perf_counter()
    plan, pieces, carry = prepare(transformer, state, vs, aus, request.layout, SOURCE["sol_dense_steps"], start=start)
    prepared = time.perf_counter()
    try:
        execution = ctx.plan(plan)
        while not execution.poll().done:
            ctx.raise_if_cancelled()
            time.sleep(0.05)
        video, audio = execution.wait()
        progress = execution.poll()
        assert not progress.canceled and progress.completed_steps == STEPS - start, progress
        slots = {} if carry is None else {key[1]: (s[0].cpu(), s[1].cpu()) for key, s in carry.slots.items()}
        facts = plan.facts()
        return {
            "video": video, "audio": audio, "slots": slots,
            "segments": ";".join(f"{p.first}:{p.end}:{'dense' if p.dense else 'sparse'}"
                                 f"{'+bootstrap' if p.bootstrap else ''}:{p.unique_rows}" for p in pieces),
            "carry_sites": 0 if carry is None else len(carry.slots),
            "prepare_ms": (prepared - began) * 1000,
            "execute_ms": (time.perf_counter() - prepared) * 1000,
            "host_graph_launches": facts["host_graph_launches"],
        }
    finally:
        plan.close()
        if carry is not None:
            carry.close()


def max_abs(a: torch.Tensor, b: torch.Tensor) -> float:
    return float((a.float() - b.float()).abs().max())


def warm(transformer, payload, ctx) -> dict[str, Any]:
    """One dense and one sparse eager step: Runtime builds what they serve; selection is reported."""
    request = Request(payload.seed, payload.latent_frames, payload.text_tokens, torch.device(str(ctx.device)))
    state = request.state()
    served = sites(transformer)
    schedulers = (copy.deepcopy(request.video_schedule), copy.deepcopy(request.audio_schedule))
    with step_invariants():
        eager(transformer, request, state, schedulers, 0, 1, None)
        sparse = request.layout(SOURCE["sol_dense_steps"])
        with torch.no_grad(), attention_scope(sparse):
            times, indices = request.rows[sparse.step]
            transformer(hidden_states=state.latents[None], audio_hidden_states=state.audio_latents[None],
                        encoder_hidden_states=request.prompt, timestep=times, timestep_indices=indices,
                        attention_kwargs={}, return_dict=False, **request.fields)
    torch.cuda.synchronize()
    return {"producer_sites": served, "backends": backends(transformer), "tokens": request.tokens}


def gate(transformer, payload, ctx, tel=None) -> dict[str, Any]:
    verify_source()
    device = torch.device(str(ctx.device))
    served = sites(transformer)
    names = set(served.values())
    report: dict[str, Any] = {
        **{k: SOURCE[k] for k in ("package_commit", "files")},
        "runtime_commit": _build_provenance.COMMIT,
        "runtime": importlib.metadata.version("cozy-runtime"),
        "tensorfs": importlib.metadata.version("tensorfs"),
        "torch": torch.__version__, "device": torch.cuda.get_device_name(device),
        "producer_sites": served, "backends": backends(transformer),
        "layers": len(transformer.transformer_blocks),
    }
    sparse_blocks = [
        f"transformer_blocks.{i}.attn" for i in range(len(transformer.transformer_blocks))
        if not any(f"transformer_blocks.{i}" == p for p in SOURCE["dense_paths"])
    ]
    missing = [path for path in sparse_blocks if path not in served]
    if not sparse_blocks or len(names) != 1 or missing:
        report.update(passed=False, reason=f"Kitchen producer not serving {missing or 'any site'}")
        return report
    (name,) = names
    request = Request(payload.seed, payload.latent_frames, payload.text_tokens, device)
    full = payload.latent_frames == video_latent_num_frames(FRAMES, 17, 5)
    report.update(producer=name, tokens=request.tokens, sparse_sites=sparse_blocks, full_15s_request=full)

    def fresh():
        return request.state(), (copy.deepcopy(request.video_schedule), copy.deepcopy(request.audio_schedule))

    def ticker(stage):
        if tel is None:
            return None
        return lambda index: tel.progress((index + 1) / STEPS, stage=stage)

    passes = {}
    for label in ("A", "B"):
        state, schedulers = fresh()
        trace: list[dict[str, Any]] = []
        with step_invariants():
            eager(transformer, request, state, schedulers, 0, STEPS, name, trace, ticker(f"eager-{label}"))
        passes[label] = (state, trace)
    a_state, a_trace = passes["A"]
    b_state, b_trace = passes["B"]
    bootstrap_steps = [t["step"] for t in a_trace if t["stats"]][:1]
    deterministic = all(x["video"] == y["video"] and x["audio"] == y["audio"] for x, y in zip(a_trace, b_trace, strict=True))
    spread = max(max_abs(a_state.latents, b_state.latents), max_abs(a_state.audio_latents, b_state.audio_latents))
    final_stats = a_trace[-1]["stats"]
    report.update(
        eager_deterministic=deterministic, eager_ab_max_abs=spread,
        eager_finite=all(t["finite"] for t in a_trace),
        eager_s_per_step=[round(t["ms"] / 1000, 3) for t in a_trace],
        eager_stats_sites_per_step=[len(t["stats"]) for t in a_trace],
        first_stats_step=bootstrap_steps,
    )
    cases = {"S1": 0, "S2": 1, "S4": 15}
    results: dict[str, dict[str, Any]] = {}
    for label in payload.cases.split(","):
        if label == "R15":
            results[label] = refusal(transformer, request, fresh, name, a_state, ticker("refusal"))
            continue
        start = cases[label]
        state, schedulers = fresh()
        try:
            with step_invariants():
                eager(transformer, request, state, schedulers, 0, start, name, tick=ticker(f"{label}-prefix"))
                got = native(transformer, request, state, schedulers, start, ctx)
        except Exception as error:  # noqa: BLE001 - a case's failure is its result, not the gate's end
            results[label] = {"start": start, "error": f"{type(error).__name__}: {error}"[:900]}
            continue
        error = max(max_abs(got["video"], a_state.latents), max_abs(got["audio"], a_state.audio_latents))
        stats_equal = {
            site: bool(torch.equal(got["slots"][site][0], mean) and torch.equal(got["slots"][site][1], scale))
            for site, (mean, scale) in final_stats.items()
        }
        results[label] = {
            "start": start, "segments": got["segments"], "carry_sites": got["carry_sites"],
            "video_exact": bool(torch.equal(got["video"], a_state.latents)),
            "audio_exact": bool(torch.equal(got["audio"], a_state.audio_latents)),
            "max_abs_vs_eager": error, "final_summaries_exact": stats_equal,
            "prepare_s": round(got["prepare_ms"] / 1000, 2), "execute_s": round(got["execute_ms"] / 1000, 2),
            "native_s_per_step": round(got["execute_ms"] / 1000 / (STEPS - start), 3),
            "host_graph_launches": got["host_graph_launches"],
            "finite": bool(got["video"].isfinite().all() and got["audio"].isfinite().all()),
        }
    report["native"] = results
    expected_segments = {"S1": "+bootstrap", "S2": "+bootstrap", "S4": None}
    ok = report["eager_finite"]
    for label, r in results.items():
        if "error" in r:
            ok = False
        elif label == "R15":
            ok &= r["refused"] and r["request_state_restored"] and r["table_forgotten"]
            ok &= r["continuation_exact"] if deterministic else r["continuation_max_abs"] <= 2 * spread
        else:
            has = "+bootstrap" in r["segments"]
            ok &= r["finite"] and has == (expected_segments[label] is not None)
            ok &= r["carry_sites"] == len(sparse_blocks)
            if deterministic:
                ok &= r["video_exact"] and r["audio_exact"] and all(r["final_summaries_exact"].values())
            else:  # bound native by eager's own run-to-run spread
                ok &= r["max_abs_vs_eager"] <= 2 * spread
    report["passed"] = bool(ok)
    return report


def same(left: Any, right: Any) -> bool:
    """Identity equality of request-state contents (tensors by object, not value)."""
    if isinstance(left, dict) and isinstance(right, dict):
        return left.keys() == right.keys() and all(same(left[k], right[k]) for k in left)
    if isinstance(left, tuple) and isinstance(right, tuple):
        return len(left) == len(right) and all(same(a, b) for a, b in zip(left, right, strict=True))
    if isinstance(left, torch.Tensor) or isinstance(right, torch.Tensor):
        return left is right
    return left == right


def refusal(transformer, request, fresh, name, a_state, tick) -> dict[str, Any]:
    """A preparation that refuses after its carry attached, with eager statistics already held
    (15 eager steps): once planning has seen every step, step 15's layout reports a different
    protected prefix, so the first sparse warm-up call refuses. The request state must come back
    exactly, Kitchen's cache must not keep the native table, and eager continues as the reference."""
    state, schedulers = fresh()
    vs, aus = schedulers
    held = producer.PRODUCERS[name].request_state()
    planned = {"done": False}

    def skewed(step: int) -> AttentionLayout:
        layout = request.layout(step)
        if step == STEPS - 1:
            planned["done"] = True
        if step == 15 and planned["done"]:
            return dataclasses.replace(layout, protected_prefix=layout.protected_prefix + 64)
        return layout

    try:
        with step_invariants():
            eager(transformer, request, state, schedulers, 0, 15, name, tick=tick)
            cache = getattr(sys.modules.get("comfy_kitchen.backends.cuda"), "_ROPE_FAB_CACHE", {})
            cached = set(cache)
            before = (dict(held.statistics), dict(held.replay_inputs), held.rope, dict(held.parallel_rotary))
            reason = ""
            try:
                plan, _, carry = prepare(transformer, state, vs, aus, skewed, SOURCE["sol_dense_steps"], start=15)
            except CapabilityError as error:
                reason = f"{error.code}: {error}"
            else:
                plan.close()
                if carry is not None:
                    carry.close()
            after = (dict(held.statistics), dict(held.replay_inputs), held.rope, dict(held.parallel_rotary))
            restored = same(before, after) and held.native is None
            forgotten = not (set(cache) - cached)
            eager(transformer, request, state, schedulers, 15, STEPS, name, tick=tick)
    except Exception as error:  # noqa: BLE001 - reported as this case's result
        return {"start": 15, "error": f"{type(error).__name__}: {error}"[:900]}
    return {
        "start": 15, "refused": bool(reason), "reason": reason[:300],
        "request_state_restored": restored, "table_forgotten": forgotten,
        "continuation_exact": bool(torch.equal(state.latents, a_state.latents)
                                   and torch.equal(state.audio_latents, a_state.audio_latents)),
        "continuation_max_abs": max(max_abs(state.latents, a_state.latents),
                                    max_abs(state.audio_latents, a_state.audio_latents)),
    }


def cpu(transformer, payload) -> dict[str, Any]:
    """Every step before CUDA, on the CPU executor: the loaded model's dtypes and attention
    selection, every scheduled row against its loaded tables, one sparse-layout eager forward,
    and native preparation up to its CUDA-only refusal."""
    verify_source()
    device = next(transformer.parameters()).device
    request = Request(payload.seed, payload.latent_frames, payload.text_tokens, device)
    report: dict[str, Any] = {
        **{k: SOURCE[k] for k in ("package_commit", "files")},
        "runtime_commit": _build_provenance.COMMIT,
        "runtime": importlib.metadata.version("cozy-runtime"),
        "tensorfs": importlib.metadata.version("tensorfs"),
        "torch": torch.__version__, "device": str(device),
        "layers": len(transformer.transformer_blocks),
        "f32_children": sorted(n for n, c in transformer.named_children()
                               if any(p.dtype == torch.float32 for p in c.parameters())),
        "parameter_dtypes": sorted({str(p.dtype) for p in transformer.parameters()}),
        "producer_sites": sites(transformer), "backends": backends(transformer),
        "attention_processors": sorted({type(m.processor).__name__ for m in transformer.modules()
                                        if hasattr(m, "processor")}),
        "tokens": request.tokens,
    }
    for times, indices in request.rows:  # the ordinary forward's own row check, on the loaded tables
        transformer._arm(transformer, (), {"timestep": times, "timestep_indices": indices,
                                           "token_tags": request.fields["token_tags"], "attention_kwargs": {}})
        transformer._release()
    report["rows_validated"] = len(request.rows)
    state = request.state()
    sparse = request.layout(SOURCE["sol_dense_steps"])
    began = time.perf_counter()
    with step_invariants(), torch.no_grad(), attention_scope(sparse):
        times, indices = request.rows[sparse.step]
        video, audio = transformer(
            hidden_states=state.latents[None], audio_hidden_states=state.audio_latents[None],
            encoder_hidden_states=request.prompt, timestep=times, timestep_indices=indices,
            attention_kwargs={}, return_dict=False, **request.fields)
    report["forward"] = {"step": sparse.step, "s": round(time.perf_counter() - began, 2),
                         "video": list(video.shape), "audio": list(audio.shape),
                         "finite": bool(video.isfinite().all() and audio.isfinite().all())}
    vs, aus = copy.deepcopy(request.video_schedule), copy.deepcopy(request.audio_schedule)
    refusal = None
    try:
        plan, _, carry = prepare(transformer, state, vs, aus, request.layout, SOURCE["sol_dense_steps"])
    except CapabilityError as error:
        refusal = error
        report["prepare_refusal"] = str(error)
    else:  # cannot happen off CUDA; release what it made
        plan.close()
        if carry is not None:
            carry.close()
    report["passed"] = bool(
        report["rows_validated"] == STEPS and len(request.video_schedule.timesteps) == STEPS
        and report["forward"]["finite"] and refusal is not None
        and refusal.code == "prepared_graph_ineligible" and "CUDA" in str(refusal)
    )
    return report
