"""The candidate Runtime's decode engine (`cozy_runtime.lm.generate`) against Transformers' own greedy `generate`, one
tiny random model of each language-model family the engine serves, on the CPU in float32: every layer full (Llama,
Qwen3), every layer sliding (Mistral and Phi-3, whose forwards take one mask; Qwen3's, which reads masks by type),
mixed by layer type (Qwen3, Ministral, Gemma 3, Gemma 4 with shared KV layers), gated delta nets (Qwen3.5). One row and
three, the first left-padded. A family whose tokens differ, or which falls back to Transformers' loop, fails: a
regression of the engine's masks or caches for one family (cl-1103: Mistral read a dict of masks) is caught here, not
by a package's cells alone.

    lm_families.py

Prints `PASS|FAIL - lm <family> <seconds>s[: why]`, one line a family."""

from __future__ import annotations

import time
import warnings
from collections.abc import Callable, Iterator

import torch
import transformers
from cozy_runtime import lm
from cozy_runtime.author._model import WARNINGS

V, PROMPT, NEW = 97, 9, 24
BASE: dict[str, int] = {"vocab_size": V, "hidden_size": 64, "intermediate_size": 128, "num_hidden_layers": 4,
                        "num_attention_heads": 4, "num_key_value_heads": 2, "head_dim": 16, "max_position_embeddings": 256}
#: Shorter than prompt and answer together, so decode slides; longer than the prompt, as Transformers' own loop needs.
WINDOW = 12
MIXED = ["sliding_attention", "full_attention"] * 2


def families() -> Iterator[tuple[str, Callable[[], torch.nn.Module]]]:
    t = transformers
    yield "llama", lambda: t.LlamaForCausalLM(t.LlamaConfig(**BASE))
    yield "mistral-sliding", lambda: t.MistralForCausalLM(t.MistralConfig(**BASE, sliding_window=WINDOW))
    yield "phi3-sliding", lambda: t.Phi3ForCausalLM(t.Phi3Config(**BASE, sliding_window=WINDOW, pad_token_id=0))
    yield "qwen3", lambda: t.Qwen3ForCausalLM(t.Qwen3Config(**BASE))
    yield "qwen3-sliding", lambda: t.Qwen3ForCausalLM(
        t.Qwen3Config(**BASE, use_sliding_window=True, sliding_window=WINDOW, max_window_layers=0))
    yield "qwen3-mixed", lambda: t.Qwen3ForCausalLM(
        t.Qwen3Config(**BASE, use_sliding_window=True, sliding_window=WINDOW, max_window_layers=2))
    yield "ministral-mixed", lambda: t.MinistralForCausalLM(t.MinistralConfig(**BASE, sliding_window=WINDOW, layer_types=MIXED))
    yield "gemma3-mixed", lambda: t.Gemma3ForCausalLM(t.Gemma3TextConfig(**BASE, sliding_window=WINDOW, layer_types=MIXED))
    yield "gemma4-mixed", lambda: t.Gemma4ForCausalLM(t.Gemma4TextConfig(
        **{**BASE, "num_hidden_layers": 6}, global_head_dim=32, num_global_key_value_heads=1, sliding_window=WINDOW,
        num_kv_shared_layers=2, hidden_size_per_layer_input=8, vocab_size_per_layer_input=V, attention_k_eq_v=True,
        layer_types=["sliding_attention", "sliding_attention", "full_attention"] * 2, final_logit_softcapping=30.0))
    yield "qwen3.5-delta", lambda: t.Qwen3_5ForCausalLM(t.Qwen3_5TextConfig(
        **BASE, linear_key_head_dim=16, linear_value_head_dim=16, linear_num_key_heads=2, linear_num_value_heads=4,
        layer_types=["linear_attention"] * 3 + ["full_attention"]))


def judge(model: torch.nn.Module) -> str:
    """What differs between the engine's tokens and Transformers', or ""."""
    for rows in (1, 3):
        torch.manual_seed(rows)
        ids = torch.randint(3, V, (rows, PROMPT))
        mask = torch.ones_like(ids)
        if rows > 1:
            mask[0, : PROMPT // 2] = 0
            ids[0, : PROMPT // 2] = 0
        inputs = {"input_ids": ids, "attention_mask": mask}
        with torch.inference_mode():
            want = model.generate(**inputs, max_new_tokens=NEW, do_sample=False, eos_token_id=[1], pad_token_id=0)  # type: ignore[operator]  # Transformers' untyped generate
        WARNINGS.clear()
        got = lm.generate(model, inputs, max_new_tokens=NEW, stop=[1], pad=0)  # type: ignore[arg-type]  # a PreTrainedModel
        if fell := next((str(w) for w in WARNINGS if "Transformers' loop" in str(w)), ""):
            return f"{rows} rows: decoded through Transformers' loop ({fell[:120]})"
        want = want[:, PROMPT:]
        n = min(want.shape[1], got.shape[1])
        if not torch.equal(want[:, :n], got[:, :n]):
            row = int((want[:, :n] != got[:, :n]).any(dim=1).nonzero()[0])
            return f"{rows} rows: row {row} differs from Transformers' greedy tokens"
    return ""


def main() -> None:
    warnings.filterwarnings("ignore")
    torch.set_num_threads(2)
    for name, build in families():
        started = time.monotonic()
        try:
            torch.manual_seed(0)
            why = judge(build().eval().float())
        except Exception as e:  # noqa: BLE001  # a family the engine breaks is this check's finding
            why = f"{type(e).__name__}: {str(e)[:160]}"
        took = time.monotonic() - started
        print(f"{'FAIL' if why else 'PASS'} - lm {name} {took:.0f}s{': ' + why if why else ''}", flush=True)


if __name__ == "__main__":
    main()
