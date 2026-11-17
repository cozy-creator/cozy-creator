"""ggml-ingest, on the pod: our ggml lanes against llama-quantize's GGUFs of the same source, tensor by tensor.

    compare.py <hf dir> <store dir> <bf16 ref> <lane>=<ref>:<gguf> ...

First the source: every tensor of the Hugging Face checkpoint llama.cpp converted equals the bf16 commit's, byte for
byte. Then each lane: every tensor llama-quantize quantized against ours, its type and its block bytes. llama.cpp's
converter reorders and merges some of Qwen3.5's (`_LinearAttentionVReorderBase`): `attn_qkv_gate` is `in_proj_qkv` (its
V rows reordered) then `in_proj_z` (reordered), `ssm_beta_alpha` is `in_proj_b` then `in_proj_a` (reordered), each row
quantized alone, so ours compare after the same row moves. `ssm_out` is `out_proj` with its columns reordered, which
moves elements between blocks: ours is checked to be TensorFS's ggml-ref/1 of `out_proj` as stored, and ggml-ref/1 of
those columns reordered against llama's. One JSON line per check; exit 1 on any difference.
"""

from __future__ import annotations

import json
import os
import struct
import sys

import numpy as np
import tensorfs
from gguf import GGUFReader
from tensorfs import raw

TYPES = {"Q8_0": ("q8_0", 32), "Q4_K": ("q4_k", 256), "Q5_K": ("q5_k", 256), "Q6_K": ("q6_k", 256),
         "Q5_0": ("q5_0", 32), "Q5_1": ("q5_1", 32)}  # fmt: skip
FLOAT = {"F32", "F16", "BF16"}


def say(**row: object) -> None:
    print(json.dumps(row), flush=True)


def safetensors(directory: str) -> dict[str, bytes]:
    """Every tensor's bytes in a Hugging Face folder's safetensors files."""
    out: dict[str, bytes] = {}
    for name in sorted(os.listdir(directory)):
        if not name.endswith(".safetensors"):
            continue
        with open(os.path.join(directory, name), "rb") as f:
            n = struct.unpack("<Q", f.read(8))[0]
            header = json.loads(f.read(n))
            base = 8 + n
            for key, info in header.items():
                if key == "__metadata__":
                    continue
                start, stop = info["data_offsets"]
                f.seek(base + start)
                out[key] = f.read(stop - start)
    return out


def main() -> int:
    hf, store, bf16_ref, *lanes = sys.argv[1:]
    config = json.load(open(os.path.join(hf, "config.json")))
    text = config.get("text_config", config)
    open_ = lambda ref: tensorfs.open(ref, local_files_only=True, cache_dir=store)  # noqa: E731
    source = open_(bf16_ref).components["model"].tensors
    failed = 0

    # The source: llama.cpp converts the same bytes as the commit holds.
    held = safetensors(hf)
    same = [k for k in held if k in source and source.read(k).tobytes() == held[k]]
    missing = sorted(set(held) - set(source))
    differ = sorted(set(held) & set(source) - set(same))
    say(check="source", hf_tensors=len(held), commit_tensors=len(source), identical=len(same), differ=differ[:10],
        missing=missing[:10])  # fmt: skip
    failed += bool(differ or missing)
    del held

    n_layer = int(text["num_hidden_layers"])
    k_heads, v_heads = int(text["linear_num_key_heads"]), int(text["linear_num_value_heads"])
    k_dim, v_dim = int(text["linear_key_head_dim"]), int(text["linear_value_head_dim"])

    def heads(n: int, head_dim: int) -> np.ndarray:
        """`_reorder_v_heads`'s permutation of `n` rows: grouped by K head to tiled."""
        return np.arange(n).reshape(k_heads, v_heads // k_heads, head_dim).transpose(1, 0, 2).reshape(-1)

    def ours(name: str) -> tuple[list[tuple[str, np.ndarray | None]], bool]:
        """llama.cpp's tensor as ours: (key, row permutation or None) in its row order; True for ssm_out's columns."""
        if name == "token_embd.weight":
            return [("model.language_model.embed_tokens.weight", None)], False
        if name == "output.weight":
            return [("lm_head.weight", None)], False
        _, bid, rest = name.split(".", 2)
        b = int(bid)
        if rest == "nextn.eh_proj.weight":
            return [("mtp.fc.weight", None)], False
        p = f"model.language_model.layers.{b}." if b < n_layer else f"mtp.layers.{b - n_layer}."
        if rest == "attn_qkv_gate.weight":
            qk = 2 * k_heads * k_dim
            qkv = np.concatenate([np.arange(qk), qk + heads(v_heads * v_dim, v_dim)])
            return [(p + "linear_attn.in_proj_qkv.weight", qkv),
                    (p + "linear_attn.in_proj_z.weight", heads(v_heads * v_dim, v_dim))], False  # fmt: skip
        if rest == "ssm_beta_alpha.weight":
            return [(p + "linear_attn.in_proj_b.weight", heads(v_heads, 1)),
                    (p + "linear_attn.in_proj_a.weight", heads(v_heads, 1))], False  # fmt: skip
        if rest == "ssm_out.weight":
            return [(p + "linear_attn.out_proj.weight", None)], True
        table = {"attn_q.weight": "self_attn.q_proj.weight", "attn_k.weight": "self_attn.k_proj.weight",
                 "attn_v.weight": "self_attn.v_proj.weight", "attn_output.weight": "self_attn.o_proj.weight",
                 "ffn_gate.weight": "mlp.gate_proj.weight", "ffn_up.weight": "mlp.up_proj.weight",
                 "ffn_down.weight": "mlp.down_proj.weight"}  # fmt: skip
        return [(p + table[rest], None)], False

    for spec in lanes:
        lane, rest = spec.split("=", 1)
        ref, path = rest.rsplit(":", 1)
        tensors = open_(ref).components["model"].tensors
        gguf = GGUFReader(path)
        encoded = {k for k, meta in tensors.items() if meta.encoding is not None}
        seen: set[str] = set()
        counts: dict[str, int] = {}
        bad: list[dict[str, object]] = []
        for t in gguf.tensors:
            kind = t.tensor_type.name
            if kind in FLOAT:
                continue
            name = TYPES[kind][0]
            counts[name] = counts.get(name, 0) + 1
            keys, columns = ours(t.name)
            want = bytes(t.data)
            pieces = []
            for key, rows in keys:
                seen.add(key)
                meta = tensors[key]
                scheme = meta.encoding.scheme if meta.encoding is not None else "plain"
                if scheme != f"ggml.{name}":
                    bad.append({"tensor": t.name, "key": key, "llama": name, "ours": scheme})
                    break
                blocks = tensors.read(key).parts["blocks"].tobytes()
                if rows is not None:
                    blocks = np.frombuffer(blocks, np.uint8).reshape(len(rows), -1)[rows].tobytes()
                pieces.append(blocks)
            else:
                got = b"".join(pieces)
                if columns:
                    # Ours is ggml-ref/1 of out_proj as stored; llama's is of out_proj's columns reordered.
                    key = keys[0][0]
                    values = np.frombuffer(source.read(key).tobytes(), np.uint16).astype(np.uint32) << 16
                    values = values.view(np.float32).reshape(meta.shape)
                    n = TYPES[kind][1]
                    again = raw.quantize(f"ggml.{name}", values.tobytes(), dtype="BF16", shape=list(values.shape),
                                         strategy="ggml-ref/1", params={"block_size": n}).parts["blocks"].tobytes()  # fmt: skip
                    if again != got:
                        bad.append({"tensor": t.name, "key": key, "ours": "not ggml-ref/1 of the source"})
                        continue
                    moved = np.ascontiguousarray(values[:, heads(v_heads * v_dim, v_dim)])
                    got = raw.quantize(f"ggml.{name}", moved.tobytes(), dtype="BF16", shape=list(moved.shape),
                                       strategy="ggml-ref/1", params={"block_size": n}).parts["blocks"].tobytes()  # fmt: skip
                if got != want:
                    a, b = np.frombuffer(got, np.uint8), np.frombuffer(want, np.uint8)
                    first = int(np.argmax(a != b)) if a.size == b.size else -1
                    bad.append({"tensor": t.name, "llama": name, "bytes": b.size, "ours_bytes": a.size,
                                "first_difference": first, "differing": int((a != b).sum()) if a.size == b.size else -1})  # fmt: skip
        extra = sorted(encoded - seen)
        say(check="lane", lane=lane, ref=ref, quantized=sum(counts.values()), types=counts,
            identical=sum(counts.values()) - len(bad), differ=bad[:20], ours_not_llamas=extra[:20])  # fmt: skip
        failed += bool(bad or extra)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
