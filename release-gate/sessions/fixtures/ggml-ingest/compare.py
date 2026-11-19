"""ggml-ingest, on the pod: our ggml lanes and TensorFS's ggml-ref/1 against llama-quantize's GGUFs of the same source.

    compare.py <hf dir> <store dir> <bf16 ref> [source] <lane>=<ref>:<gguf> ... pure:<TYPE>=<gguf> ...

`source`: every tensor of the Hugging Face checkpoint llama.cpp converted equals the bf16 commit's, byte for byte (a
fused expert stack against the commit's experts in order, each a `gate_and_up_proj`). Each lane: every tensor llama-quantize
quantized against ours, its type and its block bytes. Then each `--pure` GGUF: every tensor llama-quantize quantized
against ggml-ref/1 of the same source values, so each block type is proven, not only the ones a mix picks.

llama.cpp's converter moves some of Qwen3.5's rows (`_LinearAttentionVReorderBase`): `in_proj_qkv`'s V rows, `in_proj_z`,
`in_proj_b` and `in_proj_a` are reordered from grouped to tiled V heads; the dense converter merges `in_proj_qkv` with
`in_proj_z` (`attn_qkv_gate`) and `in_proj_b` with `in_proj_a` (`ssm_beta_alpha`); a routed expert's weights stack per
layer, a fused gate and up split in two. Each row is quantized alone, so ours compare after the same row moves.
`ssm_out` is `out_proj` with its columns reordered, which moves elements between blocks: ours is checked to be ggml-ref/1
of `out_proj` as stored, and ggml-ref/1 of those columns reordered against llama's. A tensor that differs is also judged
against ggml-ref/1 of the stored source read again here (`ours_is_ref`, `llama_is_ref`), naming the side that left it.
Each lane's tensors are also decoded, ours and llama's, by llama.cpp's own gguf-py (`gguf.quants.dequantize`), and
must be equal everywhere (`decoded_*`). A `told:<label>` lane (an earlier write, `also`) is judged on
its decoded values alone; its bytes are told.
One JSON line per check; exit 1 on any difference.
"""

from __future__ import annotations

import json
import os
import re
import struct
import sys
from collections.abc import Iterator

import numpy as np
import tensorfs
from gguf import GGMLQuantizationType, GGUFReader
from gguf.quants import dequantize
from tensorfs import raw

TYPES = {"Q8_0": ("q8_0", 32), "Q4_K": ("q4_k", 256), "Q5_K": ("q5_k", 256), "Q6_K": ("q6_k", 256),
         "Q5_0": ("q5_0", 32), "Q5_1": ("q5_1", 32)}  # fmt: skip
FLOAT = {"F32", "F16", "BF16"}
LAYER = {"attn_q.weight": "self_attn.q_proj.weight", "attn_k.weight": "self_attn.k_proj.weight",
         "attn_v.weight": "self_attn.v_proj.weight", "attn_output.weight": "self_attn.o_proj.weight",
         "ffn_gate.weight": "mlp.gate_proj.weight", "ffn_up.weight": "mlp.up_proj.weight",
         "ffn_down.weight": "mlp.down_proj.weight", "ffn_gate_shexp.weight": "mlp.shared_expert.gate_proj.weight",
         "ffn_up_shexp.weight": "mlp.shared_expert.up_proj.weight",
         "ffn_down_shexp.weight": "mlp.shared_expert.down_proj.weight"}  # fmt: skip

type Piece = tuple[str, np.ndarray | None]


def say(**row: object) -> None:
    print(json.dumps(row), flush=True)


def safetensors(directory: str) -> Iterator[tuple[str, bytes]]:
    """Each tensor's name and bytes in a Hugging Face folder's safetensors files, one at a time."""
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
                yield key, f.read(stop - start)


def main() -> int:
    hf, store, bf16_ref, *specs = sys.argv[1:]
    config = json.load(open(os.path.join(hf, "config.json")))
    text = config.get("text_config", config)
    open_ = lambda ref: tensorfs.open(ref, local_files_only=True, cache_dir=store)  # noqa: E731
    source = open_(bf16_ref).components["model"].tensors
    failed = 0

    # The source: llama.cpp converts the same bytes as the commit holds.
    seen, same, missing, differ = 0, 0, [], []
    for key, data in safetensors(hf) if "source" in specs else ():
        seen += 1
        fused = re.fullmatch(r"(.*\.mlp\.experts)\.(gate_up|down)_proj(?:\.weight)?", key)
        if key in source:
            ours_bytes = source.read(key).tobytes()
        elif fused:
            own = "gate_and_up" if fused.group(2) == "gate_up" else "down"
            parts = [f"{fused.group(1)}.{e}.{own}_proj.weight" for e in range(int(text["num_experts"]))]
            if not all(p in source for p in parts):
                missing.append(key)
                continue
            ours_bytes = b"".join(source.read(p).tobytes() for p in parts)
        else:
            missing.append(key)
            continue
        if ours_bytes == data:
            same += 1
        else:
            differ.append(key)
    if "source" in specs:
        say(check="source", hf_tensors=seen, commit_tensors=len(source), identical=same, differ=differ[:10],
            missing=missing[:10])  # fmt: skip
        failed += bool(differ or missing or not seen)
    specs = [s for s in specs if s != "source"]

    n_layer = int(text["num_hidden_layers"])
    n_expert = int(text.get("num_experts", 0) or 0)
    k_heads, v_heads = int(text["linear_num_key_heads"]), int(text["linear_num_value_heads"])
    k_dim, v_dim = int(text["linear_key_head_dim"]), int(text["linear_value_head_dim"])

    def heads(n: int, head_dim: int) -> np.ndarray:
        """`_reorder_v_heads`'s permutation of `n` rows: grouped by K head to tiled."""
        return np.arange(n).reshape(k_heads, v_heads // k_heads, head_dim).transpose(1, 0, 2).reshape(-1)

    qk = 2 * k_heads * k_dim
    qkv_rows = np.concatenate([np.arange(qk), qk + heads(v_heads * v_dim, v_dim)])

    def ours(name: str) -> tuple[list[Piece], bool]:
        """llama.cpp's tensor as ours: (key, row index or None) in its row order; True for ssm_out's columns."""
        if name == "token_embd.weight":
            return [("model.language_model.embed_tokens.weight", None)], False
        if name == "output.weight":
            return [("lm_head.weight", None)], False
        _, bid, rest = name.split(".", 2)
        b = int(bid)
        if rest == "nextn.eh_proj.weight":
            return [("mtp.fc.weight", None)], False
        p = f"model.language_model.layers.{b}." if b < n_layer else f"mtp.layers.{b - n_layer}."
        la = p + "linear_attn."
        z, beta = heads(v_heads * v_dim, v_dim), heads(v_heads, 1)
        moved: dict[str, list[Piece]] = {
            "attn_qkv_gate.weight": [(la + "in_proj_qkv.weight", qkv_rows), (la + "in_proj_z.weight", z)],
            "ssm_beta_alpha.weight": [(la + "in_proj_b.weight", beta), (la + "in_proj_a.weight", beta)],
            "attn_qkv.weight": [(la + "in_proj_qkv.weight", qkv_rows)],
            "attn_gate.weight": [(la + "in_proj_z.weight", z)],
            "ssm_beta.weight": [(la + "in_proj_b.weight", beta)],
            "ssm_alpha.weight": [(la + "in_proj_a.weight", beta)],
        }
        if rest in moved:
            return moved[rest], False
        if rest == "ssm_out.weight":
            return [(la + "out_proj.weight", None)], True
        if m := re.fullmatch(r"ffn_(gate|up|down)_exps\.weight", rest):
            pieces: list[Piece] = []
            for e in range(n_expert):
                own = f"{p}mlp.experts.{e}.{m.group(1)}_proj.weight"
                if own in source or m.group(1) == "down":
                    pieces.append((own, None))
                    continue
                fused = f"{p}mlp.experts.{e}.gate_and_up_proj.weight"
                half = int(source[fused].shape[0]) // 2
                pieces.append((fused, np.arange(half) + (half if m.group(1) == "up" else 0)))
            return pieces, False
        return [(p + LAYER[rest], None)], False

    def values(pieces: list[Piece], columns: bool) -> np.ndarray:
        """The f32 matrix llama.cpp's converter wrote for these pieces."""
        rows = []
        for key, index in pieces:
            v = (np.frombuffer(source.read(key).tobytes(), np.uint16).astype(np.uint32) << 16).view(np.float32)
            v = v.reshape(int(source[key].shape[0]), -1)
            rows.append(v if index is None else v[index])
        whole = np.concatenate(rows) if len(rows) > 1 else rows[0]
        return np.ascontiguousarray(whole[:, heads(v_heads * v_dim, v_dim)] if columns else whole)

    def ref(name: str, block: int, matrix: np.ndarray) -> bytes:
        """ggml-ref/1 of `matrix` in `name`, 4096 rows at a time (rows encode alone)."""
        out = []
        for at in range(0, matrix.shape[0], 4096):
            part = matrix[at : at + 4096]
            encoded = raw.quantize(f"ggml.{name}", part.tobytes(), dtype="BF16", shape=list(part.shape),
                                   strategy="ggml-ref/1", params={"block_size": block})  # fmt: skip
            out.append(encoded.parts["blocks"].tobytes())
        return b"".join(out)

    def decoded(name: str, data: bytes) -> np.ndarray:
        """Block bytes' values as llama.cpp's own gguf-py decodes them (`gguf.quants.dequantize`)."""
        return dequantize(np.frombuffer(data, np.uint8), GGMLQuantizationType[name.upper()]).ravel()

    def differs(tensor: str, name: str, got: bytes, want: bytes) -> dict[str, object] | None:
        if got == want:
            return None
        a, b = np.frombuffer(got, np.uint8), np.frombuffer(want, np.uint8)
        first = int(np.argmax(a != b)) if a.size == b.size else -1
        return {"tensor": tensor, "llama": name, "bytes": b.size, "ours_bytes": a.size, "first_difference": first,
                "differing": int((a != b).sum()) if a.size == b.size else -1}  # fmt: skip

    for spec in specs:
        label, rest = spec.split("=", 1)
        told = label.startswith("told:")
        label = label.removeprefix("told:")
        pure = label.startswith("pure:")
        if pure:
            path, tensors, ref_ = rest, None, None
        else:
            ref_, path = rest.rsplit(":", 1)
            tensors = open_(ref_).components["model"].tensors
        gguf = GGUFReader(path)
        seen_keys: set[str] = set()
        counts: dict[str, int] = {}
        bad: list[dict[str, object]] = []
        compared, unequal = 0, []  # decoded
        for t in gguf.tensors:
            kind = t.tensor_type.name
            if kind in FLOAT:
                continue
            name, block = TYPES[kind]
            counts[name] = counts.get(name, 0) + 1
            pieces, columns = ours(t.name)
            want = bytes(t.data)
            if pure:
                found = differs(t.name, name, ref(name, block, values(pieces, columns)), want)
                if found:
                    bad.append(found)
                continue
            assert tensors is not None
            blocks = []
            for key, index in pieces:
                seen_keys.add(key)
                meta = tensors[key]
                scheme = meta.encoding.scheme if meta.encoding is not None else "plain"
                if scheme != f"ggml.{name}":
                    bad.append({"tensor": t.name, "key": key, "llama": name, "ours": scheme})
                    break
                data = tensors.read(key).parts["blocks"].tobytes()
                if index is not None:
                    data = np.frombuffer(data, np.uint8).reshape(int(meta.shape[0]), -1)[index].tobytes()
                blocks.append(data)
            else:
                got = b"".join(blocks)
                if columns:
                    # Ours is ggml-ref/1 of out_proj as stored; llama's is of out_proj's columns reordered.
                    if ref(name, block, values(pieces, False)) != got:
                        bad.append({"tensor": t.name, "key": pieces[0][0], "ours": "not ggml-ref/1 of the source"})
                        continue
                    got = ref(name, block, values(pieces, True))
                compared += 1
                if not np.array_equal(decoded(name, got), decoded(name, want)):
                    unequal.append(t.name)
                if found := differs(t.name, name, got, want):
                    # Which side left the source, for the first few that differ (a re-encode of a 285 MB expert stack
                    # takes minutes): ggml-ref/1 of the source as stored here, read now, against both. Ours alone off it
                    # is our quantize's read or write; llama's alone off it is llama's or the encoder's.
                    if len(bad) < 3:
                        again = got if columns else ref(name, block, values(pieces, False))
                        found |= {"ours_is_ref": got == again, "llama_is_ref": want == again}
                        if not found["llama_is_ref"]:
                            a, b = np.frombuffer(again, np.uint8), np.frombuffer(want, np.uint8)
                            at = int(np.argmax(a != b))
                            found |= {"ref_vs_llama_first": at, "ref_block": a[at - at % 34 : at - at % 34 + 34].tolist(),
                                      "llama_block": b[at - at % 34 : at - at % 34 + 34].tolist()}  # fmt: skip
                    bad.append(found)
                    print(json.dumps({"progress": t.name, "differs": found}), file=sys.stderr, flush=True)
        row: dict[str, object] = {"check": "pure" if pure else "lane", "lane": label, "gguf": os.path.basename(path),
                                  "quantized": sum(counts.values()), "types": counts,
                                  "identical": sum(counts.values()) - len(bad), "differ": bad[:20]}  # fmt: skip
        if tensors is not None:
            encoded = {k for k, meta in tensors.items() if meta.encoding is not None}
            row |= {"ref": ref_, "ours_not_llamas": sorted(encoded - seen_keys)[:20], "told": told,
                    "decoded_compared": compared, "decoded_equal": compared - len(unequal), "decoded_differ": unequal[:20]}  # fmt: skip
            failed += bool(row["ours_not_llamas"]) or bool(unequal)
        say(**row)
        failed += (bool(bad) and not told) or not counts
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
