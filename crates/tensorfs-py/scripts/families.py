#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy"]
# ///
"""Generate the family registry (`crates/tensorfs/families/*.json`) from each family's pinned reference.

    HF_TOKEN=… uv run scripts/families.py           # regenerate every family and compare byte for byte
    HF_TOKEN=… uv run scripts/families.py --write   # rewrite them

Reads only safetensors headers and, for the layout probe, a few row blocks, by HTTP range. A
component's keys are its reference header's in file order; where a family's converter is the
only way to its canonical form (Ideogram 4's fused Q/K/V, SeedVR2's pickles), they are that
converter's output. Every converter table of a family must produce one of its forms, and every
LoRA table row must target one of its weights: checked here, so the registry and the tables
cannot drift apart. Forms and trained-on fields are spec data below, each with its source.
"""

from __future__ import annotations

import argparse
import base64
import json
import math
import os
import struct
import sys
import urllib.request
from pathlib import Path
from typing import Any

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT.parent / "tensorfs" / "families"
TABLES = ROOT.parent / "tensorfs" / "converters"
VECTORS = ROOT.parent / "tensorfs" / "vectors"
HF = "https://huggingface.co"

# The probe (src/family/probe.rs): per probed tensor, BLOCKS evenly spaced ROWS-row blocks, each
# sketched as P·(W_b·r). Thresholds measured on H3 (outputs/opus-takeover-20261027/tfs-137):
# fine-tunes ≥ 0.998 on every block, other blocks' rows ≤ 0.54, the nearer variant 6–10× nearer.
PROBE = {"seed": 137, "rows": 128, "dims": 32, "min_cosine": 0.9, "distinct": 3.0}
BLOCKS = 8

H3 = "hf://MiniMaxAI/MiniMax-H3@42ed227ee7df40d41602854ae760620d6eb651fe"
H3_PROBED = [
    f"{layer}.{t}"
    for layer in ("transformer_blocks.0", "token_refiner.refiner_blocks.0")
    for t in ("attn.to_q.weight", "attn.to_k.weight", "attn.to_v.weight", "ff.net.0.proj.weight")
]
H3_DIT = ["transformer", "transformer_ref"]

FAMILIES: list[dict[str, Any]] = [
    {
        "name": "minimax-h3",
        "reference": H3,
        "folders": ["transformer", "transformer_ref", "vae", "audio_vae", "text_encoder"],
        "classes": ["MiniMaxH3ModularPipeline", "MiniMaxH3Transformer3DModel"],
        "sources": ["h3.native", "h3.comfy", "h3.curve", "h3.lora"],
        # ComfyUI's curve DiT (comfy/ldm/minimax/model.py `use_adaln_curves`, h3.curve), and the
        # served AdaLN-pruned lanes (packages minimax-h3-tools kernel.py `table_shapes`).
        "forms": {
            "curve": {
                "components": H3_DIT,
                "drop": ["time_embedder.*"],
                "set": [["transformer_blocks.*.adaln_proj.linear.weight", [96768, 8]], ["norm_out.linear.weight", [10752, 8]]],
                "add": [["adaln_t_table", [1025, 8]]],
            },
            "pruned": {
                "components": H3_DIT,
                "drop": ["time_embedder.*", "norm_out.linear.*", "transformer_blocks.*.adaln_proj.linear.*"],
                "add": [[f"transformer_blocks.{n}.adaln_proj.table", [None, 6, 5376]] for n in range(50)]
                + [["norm_out.table", [None, 2, 5376]]],
            },
        },
        "form_sizes": {"canonical": 638, "curve": 635, "pruned": 583},
        # h3.lora's LoRA-only AdaLN-curve sites (#2245): factors over the curve's k coordinates, folded into the
        # served tables, never a weight.
        "sites": {"transformer": [[f"transformer_blocks.{n}.adaln_curve", [96768, 8]] for n in range(50)]},
        "probe": {"component": "transformer", "keys": H3_PROBED, "variants": {"transformer": "transformer", "transformer_ref": "transformer_ref"}},
        # musubi-tuner (AkaneTendo25@0499c2f8 minimax_h3_train_network.py `extra_metadata`): the
        # mode picks the DiT it trains. ai-toolkit (ostris@b69b9831 minimax_h3.py): only the
        # ref2va classes are decisive; arch `minimax_h3` trains either partition.
        "trained_on": [
            {"producer": "musubi-tuner", "field": "ss_h3_training_mode",
             "values": {"fl2va": "transformer", "ref2va": "transformer_ref", "ref2va_omni": "transformer_ref"}},
            {"producer": "ai-toolkit", "field": "ss_base_model_version",
             "values": {"minimax_h3_ref2va": "transformer_ref", "minimax_h3_omni": "transformer_ref"}},
        ],
    },
    {
        "name": "sdxl",
        "reference": "hf://stabilityai/stable-diffusion-xl-base-1.0@462165984030d82259a11f4367a4eed129e94a7b",
        "folders": ["unet", "vae", "text_encoder", "text_encoder_2"],
        "classes": ["StableDiffusionXLPipeline"],
        "sources": ["sdxl.single_file", "sdxl.lora"],
        "optional": {c: ["text_model.embeddings.position_ids"] for c in ("text_encoder", "text_encoder_2")},
        "probe": {
            "component": "text_encoder_2",
            "keys": [f"text_model.encoder.layers.0.self_attn.{p}_proj.weight" for p in "qkv"] + ["text_projection.weight"],
            "variants": {"text_encoder_2": "text_encoder_2"},
        },
    },
    {
        "name": "anima",
        "reference": "hf://circlestone-labs/Anima-Base-v1.0-Diffusers@073c3a9db359c31ad0e8aa268d15775473c2176c",
        "classes": ["AnimaTextConditioner"],
        "folders": ["transformer", "text_conditioner", "text_encoder", "vae"],
        "sources": ["anima.single_file"],
    },
    {
        "name": "qwen-image",
        "reference": "hf://Qwen/Qwen-Image-2512@25468b98e3276ca6700de15c6628e51b7de54a26",
        "classes": ["QwenImagePipeline", "QwenImageEditPlusPipeline", "QwenImageTransformer2DModel"],
        "folders": ["transformer", "text_encoder", "vae"],
    },
    {
        "name": "qwen-image-2",
        "reference": "hf://Qwen/Qwen-Image-2.1@790c92633540aa0cb11d9abf19eb46d861714758",
        "classes": ["QwenImage21Pipeline", "QwenImage21Transformer2DModel"],
        "folders": ["transformer", "text_encoder", "vae"],
    },
    {
        "name": "krea-2",
        "reference": "hf://krea/Krea-2-Raw@6b0ece7fffb640c5e3bcbe0a7f10f66b8e60a603",
        "classes": ["Krea2Pipeline", "Krea2Transformer2DModel"],
        "folders": ["transformer", "text_encoder", "vae"],
        "sources": ["krea2.lora"],
    },
    {
        "name": "moss",
        "reference": "hf://OpenMOSS-Team/MOSS-Image-SFT@f972bd4b4b9e3852133cc5da954d87bc32cd8fb9",
        "classes": ["MOSSImagePipeline", "MOSSImageTransformer"],
        "folders": ["transformer", "text_encoder", "vae"],
    },
    {
        "name": "ideogram-4",
        "reference": "hf://ideogram-ai/ideogram-4-fp8@ee79a7237b519f1402ceacf952f30c8a31ec5073",
        "classes": ["Ideogram4Pipeline", "Ideogram4Transformer2DModel"],
        "folders": ["transformer", "unconditional_transformer", "text_encoder", "vae"],
        "sources": ["ideogram4.fp8"],
        # The fp8 reference's fused Q/K/V is split by its table: that output is canonical.
        "converted": {"transformer": "ideogram4.fp8", "unconditional_transformer": "ideogram4.fp8"},
        "probe": {
            "component": "transformer",
            "keys": [f"layers.0.attention.to_{p}.weight" for p in "qkv"],
            "variants": {"transformer": "transformer", "unconditional_transformer": "unconditional_transformer"},
        },
    },
    {
        "name": "seedvr2",
        # Pickles: the components are its table's (the package's fp16 checkpoints, whose keys
        # converters.py checks against NaDiT's and the VAE's construction).
        "reference": "hf://ByteDance-Seed/SeedVR2-7B@eb0c4281d41ba3767d4f14370f0e37e9e9180c16",
        "folders": [],
        "sources": ["seedvr2"],
        "banked": {"dit": "seedvr2-headers/seedvr2_ema_7b_fp16.safetensors.header", "vae": "seedvr2-headers/ema_vae_fp16.safetensors.header"},
    },
]

# --------------------------------------------------------------------------------- reading

TOKEN = os.environ.get("HF_TOKEN", "")


def get(url: str, rng: tuple[int, int] | None = None) -> bytes:
    headers = {"Authorization": f"Bearer {TOKEN}"} if TOKEN else {}
    if rng:
        headers["Range"] = f"bytes={rng[0]}-{rng[1] - 1}"
    for attempt in range(5):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=300) as r:
                body = r.read()
            if rng is None or len(body) == rng[1] - rng[0]:
                return body
        except Exception as e:  # noqa: BLE001
            err = e
    sys.exit(f"{url}: {err}")


def split(reference: str) -> tuple[str, str]:
    repo, commit = reference.removeprefix("hf://").split("@")
    return repo, commit


def tree(reference: str) -> list[dict[str, Any]]:
    repo, commit = split(reference)
    return json.loads(get(f"{HF}/api/models/{repo}/tree/{commit}?recursive=true"))


def url(reference: str, path: str) -> str:
    repo, commit = split(reference)
    return f"{HF}/{repo}/resolve/{commit}/{path}"


HEADERS: dict[str, tuple[int, dict[str, Any]]] = {}


def header(u: str) -> tuple[int, dict[str, Any]]:
    if u not in HEADERS:
        (n,) = struct.unpack("<Q", get(u, (0, 8)))
        HEADERS[u] = (8 + n, json.loads(get(u, (8, 8 + n))))
    return HEADERS[u]


def folder_files(files: list[dict[str, Any]], folder: str) -> list[str]:
    """A folder's weights: its index's shards, else its one file; the default variant first."""
    names = [f["path"] for f in files if f["type"] == "file" and f["path"].rsplit("/", 1)[0] == folder]
    weights = [n for n in names if n.endswith(".safetensors")]
    plain = [n for n in weights if ".fp16." not in n and ".bf16." not in n] or weights
    return sorted(plain)


def component(reference: str, files: list[dict[str, Any]], folder: str) -> tuple[list[tuple[str, list[int]]], dict[str, tuple[str, int]]]:
    """(key, shape) in file order, and where each key's bytes are: (url, absolute offset)."""
    keys, at = [], {}
    for path in folder_files(files, folder):
        u = url(reference, path)
        base, h = header(u)
        rows = sorted(((k, v) for k, v in h.items() if k != "__metadata__"), key=lambda kv: kv[1]["data_offsets"][0])
        for k, v in rows:
            keys.append((k, v["shape"]))
            at[k] = (u, base + v["data_offsets"][0], v["dtype"])
    return keys, at


def class_of(reference: str, files: list[dict[str, Any]], folder: str) -> str | None:
    paths = {f["path"] for f in files}
    index = "model_index.json"
    if index in paths:
        entry = json.loads(get(url(reference, index))).get(folder)
        if isinstance(entry, list) and len(entry) >= 2 and isinstance(entry[1], str):
            return entry[1]
    config = f"{folder}/config.json"
    if config in paths:
        c = json.loads(get(url(reference, config)))
        return c.get("_class_name") or (c.get("architectures") or [None])[0]
    return None


def banked(path: str) -> list[tuple[str, list[int]]]:
    raw = (VECTORS / path).read_bytes()
    (n,) = struct.unpack("<Q", raw[:8])
    h = json.loads(raw[8 : 8 + n])
    rows = sorted(((k, v) for k, v in h.items() if k != "__metadata__"), key=lambda kv: kv[1]["data_offsets"][0])
    return [(k, v["shape"]) for k, v in rows]


# --------------------------------------------------------------------------------- tables

def table(name: str) -> dict[str, Any] | None:
    path = TABLES / f"{name}.json"
    return json.loads(path.read_text()) if path.exists() else None


def expand(rows: list[Any]) -> list[list[Any]]:
    """A table's rows with `{"e": n}` repetition expanded (engine.rs `rows`)."""
    out = []
    for r in rows:
        if isinstance(r, list) and len(r) == 4 and isinstance(r[3], dict):
            texts = [json.dumps(r[:3])]
            for mark, n in r[3].items():
                texts = [t.replace(f'"{{{mark}}}"', str(i)).replace(f"{{{mark}}}", str(i)) for t in texts for i in range(n)]
            out.extend(json.loads(t) for t in texts)
        else:
            out.append(r)
    return out


def resolve(dims: list[int], numel: int) -> list[int]:
    known = math.prod(d for d in dims if d > 0)
    return [numel // known if d == -1 else d for d in dims]


def shaped(shape: list[int], ops: dict[str, Any]) -> list[int]:
    """The output shape of a row's ops over `shape` (engine.rs `layout`)."""
    numel = math.prod(shape)
    dims = resolve(ops["view"], numel) if "view" in ops else list(shape)
    if "take" in ops:
        axis, idx = ops["take"]
        dims[axis] = len(idx)
    if "permute" in ops:
        dims = [dims[i] for i in ops["permute"]]
    if "shape" in ops:
        dims = resolve(ops["shape"], math.prod(dims))
    return dims


def rows_of(source_rows: int, ops: dict[str, Any]) -> np.ndarray:
    """The source row each output row reads, for ops that select whole rows."""
    a = np.arange(source_rows).reshape(source_rows, 1)
    if "view" in ops:
        a = a.reshape(ops["view"])
    if "take" in ops:
        a = np.take(a, ops["take"][1], axis=ops["take"][0])
    if "shape" in ops:
        a = a.reshape(ops["shape"])
    assert a.ndim == 2 and a.shape[1] == 1, ops
    return a[:, 0]


def converted(t: dict[str, Any], comp: str, source: list[tuple[str, list[int]]]) -> list[tuple[str, list[int]]]:
    shapes = dict(source)
    (c,) = [c for c in t["components"] if c["name"] == comp or comp in c.get("folders", {}).values()]
    out = []
    for r in expand(c["rows"]):
        target, src, ops = r[0], (r[1] if len(r) > 1 else r[0]), (r[2] if len(r) > 2 else {})
        out.append((target, shaped(shapes[c.get("prefix", "") + src], ops) if ops else shapes[c.get("prefix", "") + src]))
    return out


# --------------------------------------------------------------------------------- forms

def glob(pattern: str, text: str) -> bool:
    parts = pattern.split("*")
    if len(parts) == 1:
        return pattern == text
    if not (text.startswith(parts[0]) and text[len(parts[0]):].endswith(parts[-1])):
        return False
    rest = text[len(parts[0]) : len(text) - len(parts[-1])]
    for middle in parts[1:-1]:
        at = rest.find(middle)
        if at < 0:
            return False
        rest = rest[at + len(middle) :]
    return True


EXPECTED: dict[tuple[str, str, str], dict[str, list[int | None]]] = {}


def expected(fam: dict[str, Any], comp: str, form: str) -> dict[str, list[int | None]]:
    key = (fam["name"], comp, form)
    if key not in EXPECTED:
        EXPECTED[key] = edited(fam, comp, form)
    return EXPECTED[key]


def edited(fam: dict[str, Any], comp: str, form: str) -> dict[str, list[int | None]]:
    root = fam["components"][comp].get("same_as", comp)
    keys = {k: list(s) for k, s in fam["components"][root]["keys"]}
    edit = fam.get("forms", {}).get(form)
    if form == "canonical" or comp not in edit["components"] and root not in edit["components"]:
        return keys
    keys = {k: s for k, s in keys.items() if not any(glob(p, k) for p in edit.get("drop", []))}
    for pattern, shape in edit.get("set", []):
        for k in keys:
            if glob(pattern, k):
                keys[k] = shape
    keys.update({k: s for k, s in edit.get("add", [])})
    return keys


def fits(want: list[int | None], got: list[int]) -> bool:
    return len(want) == len(got) and all(w is None or w == g for w, g in zip(want, got))


def check_tables(fam: dict[str, Any]) -> None:
    forms = ["canonical", *fam.get("forms", {})]
    for name in fam.get("sources", []):
        t = table(name)
        if t is None:
            print(f"  {fam['name']}: table {name} is not in this tree; not checked")
            continue
        for c in t["components"]:
            outs = sorted({*c.get("folders", {}).values()} or {c["name"]})
            for comp in outs:
                if comp not in fam["components"]:
                    sys.exit(f"{name}: component {comp} is not {fam['name']}'s")
                targets = [r[0] for r in expand(c["rows"])]
                if t.get("lora"):
                    for r in expand(c["rows"]):
                        i, o = r[2]["io"]
                        # The B factor's ops cut a fused module's rows to the target's.
                        o = shaped([o, 1], r[2]["b"])[0] if "b" in r[2] else o
                        want = [o, i, *r[2].get("kernel", [])]
                        ok = any(fits(expected(fam, comp, f).get(f"{r[0]}.weight", []), want) for f in forms) or any(
                            p == r[0] and s == want for p, s in fam["components"][comp].get("sites", []))
                        if not ok:
                            sys.exit(f"{name}: LoRA row {r[0]} {want} targets no {fam['name']} {comp} weight")
                    continue
                optional = set(fam["components"].get(comp, {}).get("optional", []))
                matched = [f for f in forms if set(targets) - optional == set(expected(fam, comp, f)) - optional]
                if not matched:
                    sys.exit(f"{name}: {comp}'s targets are no {fam['name']} form")
                print(f"  {fam['name']}: {name} writes {comp} as {matched[0]}")


# --------------------------------------------------------------------------------- probe

def splitmix64(x: np.ndarray) -> np.ndarray:
    with np.errstate(over="ignore"):
        z = x + np.uint64(0x9E3779B97F4A7C15)
        z = (z ^ (z >> np.uint64(30))) * np.uint64(0xBF58476D1CE4E5B9)
        z = (z ^ (z >> np.uint64(27))) * np.uint64(0x94D049BB133111EB)
        return z ^ (z >> np.uint64(31))


def signs(seed: int, n: int) -> np.ndarray:
    x = (np.uint64(seed) << np.uint64(32)) + np.arange(n, dtype=np.uint64)
    return np.where(splitmix64(x) >> np.uint64(63) == 1, -1.0, 1.0)


def e4m3(b: int) -> float:
    e, m = (b >> 3) & 15, b & 7
    v = (1 + m / 8) * 2.0 ** (e - 7) if e else m / 8 * 2.0**-6
    return -v if b & 0x80 else v


E4M3 = np.array([e4m3(b) for b in range(256)])
E4M3[[0x7F, 0xFF]] = np.nan


def decode(dtype: str, raw: bytes, cols: int) -> np.ndarray:
    if dtype == "BF16":
        a = (np.frombuffer(raw, np.uint16).astype(np.uint32) << 16).view(np.float32)
    elif dtype == "F16":
        a = np.frombuffer(raw, np.float16)
    elif dtype == "F32":
        a = np.frombuffer(raw, np.float32)
    elif dtype == "F8_E4M3":
        a = E4M3[np.frombuffer(raw, np.uint8)]
    else:
        sys.exit(f"probe: dtype {dtype}")
    return a.astype(np.float64).reshape(-1, cols)


def read_rows(where: tuple[str, int, str], cols: int, rows: np.ndarray) -> np.ndarray:
    u, start, dtype = where
    size = {"BF16": 2, "F16": 2, "F32": 4, "F8_E4M3": 1}[dtype]
    out, i = [], 0
    while i < len(rows):
        j = i
        while j + 1 < len(rows) and rows[j + 1] == rows[j] + 1:
            j += 1
        a = start + int(rows[i]) * cols * size
        out.append(decode(dtype, get(u, (a, a + (j - i + 1) * cols * size)), cols))
        i = j + 1
    return np.concatenate(out)


def sketch(w: np.ndarray) -> np.ndarray:
    y = w @ signs(PROBE["seed"], w.shape[1])
    return signs(PROBE["seed"] + 1, PROBE["dims"] * len(y)).reshape(PROBE["dims"], len(y)) @ y


def probe(fam: dict[str, Any], where: dict[str, dict[str, tuple[str, int, str]]], shapes: dict[str, dict[str, list[int]]]) -> dict[str, Any]:
    spec = fam["probe"]
    tensors = []
    for key in spec["keys"]:
        rows_total, cols = shapes[spec["component"]][key][0], math.prod(shapes[spec["component"]][key][1:])
        n = rows_total // PROBE["rows"]
        blocks = sorted({round(x) for x in np.linspace(0, n - 1, BLOCKS)})
        out = {}
        for variant, folder in spec["variants"].items():
            reader = where[folder][key]
            sketches = []
            for b in blocks:
                rows = reader["rows"][b * PROBE["rows"] : (b + 1) * PROBE["rows"]]
                w = read_rows(reader["at"], reader["cols"], rows)
                if reader.get("scale") is not None:
                    w = w * read_rows(reader["scale"], 1, rows)
                sketches.append(sketch(w))
            out[variant] = base64.b64encode(np.concatenate(sketches).astype("<f2").tobytes()).decode()
            print(f"  {fam['name']}: probe {key} ({variant}) {len(blocks)} blocks")
        tensors.append({"component": spec["component"], "key": key, "blocks": blocks, "sketch": out})
    return {**PROBE, "tensors": tensors}


def readers(fam: dict[str, Any], folder: str, at: dict[str, tuple[str, int, str]], source: list[tuple[str, list[int]]]) -> dict[str, dict[str, Any]]:
    """How each canonical key's rows are read from the reference: directly, or through the
    family's converter (each output row a source row; an fp8 row with its scale)."""
    shapes = dict(source)
    name = fam.get("converted", {}).get(folder)
    if name is None:
        return {k: {"at": at[k], "cols": math.prod(s[1:]), "rows": np.arange(s[0])} for k, s in source if len(s) >= 2}
    t = table(name)
    (c,) = [c for c in t["components"] if folder in c.get("folders", {}).values() or c["name"] == folder]
    out = {}
    for r in expand(c["rows"]):
        target, src, ops = r[0], (r[1] if len(r) > 1 else r[0]), (r[2] if len(r) > 2 else {})
        s = shapes[src]
        if len(s) < 2:
            continue
        scale = at.get(src.removesuffix(".weight") + ".weight_scale")
        out[target] = {"at": at[src], "cols": math.prod(s[1:]), "rows": rows_of(s[0], ops) if ops else np.arange(s[0]), "scale": scale}
    return out


# --------------------------------------------------------------------------------- families

def build(spec: dict[str, Any]) -> dict[str, Any]:
    print(spec["name"])
    files = tree(spec["reference"]) if spec["folders"] else []
    comps: dict[str, Any] = {}
    where, shapes = {}, {}
    for folder in spec["folders"]:
        source, at = component(spec["reference"], files, folder)
        name = spec.get("converted", {}).get(folder)
        keys = converted(table(name), folder, source) if name else source
        where[folder] = readers(spec, folder, at, source)
        shapes[folder] = dict(keys)
        mine = sorted((k, tuple(s)) for k, s in keys)
        same = next((c for c, d in comps.items() if "keys" in d and sorted((k, tuple(s)) for k, s in d["keys"]) == mine), None)
        if same:
            comps[folder] = {"same_as": same}
            continue
        comps[folder] = {"class": class_of(spec["reference"], files, folder), "keys": [[k, s] for k, s in keys]}
        if folder in spec.get("optional", {}):
            comps[folder]["optional"] = spec["optional"][folder]
    # LoRA-only targets a family's adapters train that no weight is: [path, [out, in]].
    for comp, sites in spec.get("sites", {}).items():
        comps[comp]["sites"] = sites
    for comp, path in spec.get("banked", {}).items():
        comps[comp] = {"keys": [[k, s] for k, s in banked(path)]}
    if spec["name"] == "seedvr2":
        t = table("seedvr2")
        (cond,) = [c for c in t["components"] if c["name"] == "conditioning"]
        comps["conditioning"] = {"keys": [[r[0], r[1]["shape"]] for r in cond["rows"]]}
    for d in comps.values():
        if d.get("class") is None:
            d.pop("class", None)
    fam: dict[str, Any] = {"name": spec["name"], "reference": spec["reference"]}
    for field in ("sources", "classes"):
        if spec.get(field):
            fam[field] = spec[field]
    fam["components"] = comps
    if spec.get("forms"):
        fam["forms"] = spec["forms"]
    for form, n in spec.get("form_sizes", {}).items():
        got = len(expected(fam, "transformer", form))
        assert got == n, f"{spec['name']} {form}: {got} keys, {n} expected"
    check_tables(fam)
    if spec.get("probe"):
        fam["probe"] = probe(spec | fam, where, shapes)
    if spec.get("trained_on"):
        fam["trained_on"] = spec["trained_on"]
    return fam


def render(fam: dict[str, Any]) -> str:
    return json.dumps(fam, separators=(",", ":")) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--write", action="store_true", help="rewrite the families")
    parser.add_argument("names", nargs="*", help="only these families")
    args = parser.parse_args()
    OUT.mkdir(exist_ok=True)
    differ = []
    for spec in FAMILIES:
        if args.names and spec["name"] not in args.names:
            continue
        text = render(build(spec))
        path = OUT / f"{spec['name']}.json"
        if args.write:
            path.write_text(text)
        elif not path.exists() or path.read_text() != text:
            differ.append(spec["name"])
    if differ:
        sys.exit(f"differ from their references: {', '.join(differ)} (--write to regenerate)")


if __name__ == "__main__":
    main()
