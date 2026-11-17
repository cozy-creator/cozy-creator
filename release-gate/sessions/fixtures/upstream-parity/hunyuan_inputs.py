"""HunyuanImage 3.0's model inputs, as one code tree builds them: its tokenizer and image processor on the checkpoint's own
files, then `apply_chat_template` with the arguments `preprocess_inputs` passes (modeling_hunyuan_image_3.py), for each
case below. No weights. Run once in Tencent's environment on Tencent's tree (Transformers 4.57.1) and once in the
package's on its vendored tree (Transformers 5.23), then `compare` the two dumps.

  python hunyuan_inputs.py dump <tree: hunyuan_image_3 dir> <files dir> <upstream|package> <out.pt>
  python hunyuan_inputs.py compare <a.pt> <b.pt>"""

import importlib.util
import json
import sys
import types
from pathlib import Path

import numpy as np
import torch


def tree_modules(path: Path) -> types.ModuleType:
    """The tree as package `hy`, without running its `__init__` (Tencent's imports a top-level `utils`)."""
    package = types.ModuleType("hy")
    package.__path__ = [str(path)]
    sys.modules["hy"] = package
    for name in ("configuration_hunyuan_image_3", "tokenization_hunyuan_image_3", "system_prompt", "image_processor"):
        spec = importlib.util.spec_from_file_location(f"hy.{name}", path / f"{name}.py")
        module = importlib.util.module_from_spec(spec)
        sys.modules[f"hy.{name}"] = module
        spec.loader.exec_module(module)
        setattr(package, name, module)
    return package


def pictures():
    rng = np.random.default_rng(7)
    from PIL import Image
    rgb = Image.fromarray(rng.integers(0, 256, (700, 900, 3), dtype=np.uint8), "RGB")
    rgba = rng.integers(0, 256, (960, 640, 4), dtype=np.uint8)
    rgba[:300, :, 3] = 0  # a transparent band, its colour arbitrary
    return rgb, Image.fromarray(rgba, "RGBA")


def plain(value):
    """A dump's leaves: tensors as they are, slices and dataclass-likes as nested plain values."""
    if isinstance(value, torch.Tensor):
        # An ImageTensor carries its ImageInfo and encoder kwargs as attributes.
        attrs = {k: plain(v) for k, v in vars(value).items() if not k.startswith("_") and not callable(v)}
        tensor = value.detach().cpu().as_subclass(torch.Tensor)
        return {"tensor": tensor, "attrs": attrs} if attrs else tensor
    if isinstance(value, slice):
        return ("slice", value.start, value.stop, value.step)
    if isinstance(value, dict):
        return {str(k): plain(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [plain(v) for v in value]
    if hasattr(value, "__dict__") and not isinstance(value, type):
        fields = {k: plain(v) for k, v in vars(value).items() if not k.startswith("_") and not callable(v)}
        return {"<" + type(value).__name__ + ">": fields}
    if isinstance(value, (int, float, str, bool)) or value is None:
        return value
    return repr(value)


def dump(tree: Path, files: Path, side: str, out: Path) -> None:
    hy = tree_modules(tree)
    raw = json.loads((files / "config.json").read_text())
    config = hy.configuration_hunyuan_image_3.HunyuanImage3Config(**raw)
    generation = json.loads((files / "generation_config.json").read_text())
    Tok = hy.tokenization_hunyuan_image_3.HunyuanImage3TokenizerFast
    if side == "upstream":  # Tencent's `load_tokenizer`
        tokenizer = Tok.from_pretrained(str(files), model_version=getattr(config, "model_version", None) or "HunyuanImage-3.0-Instruct")
    else:  # the package's model.py `_tokenizer`
        from tokenizers import Tokenizer
        tc = json.loads((files / "tokenizer_config.json").read_text())
        tc.pop("tokenizer_class", None)
        tokenizer = Tok(tokenizer_object=Tokenizer.from_str((files / "tokenizer.json").read_text()), **tc)
    processor = hy.image_processor.HunyuanImage3ImageProcessor(config)
    system = hy.system_prompt.get_system_prompt("en_unified", "think_recaption").strip()
    rgb, rgba = pictures()
    cot = "<think>A fox, low sun, snow.</think><recaption>A red fox stands in fresh snow at sunset , photo.</recaption>"
    cases = {
        "think-t2i": dict(mode="gen_text", images=[], bot_task="think", cot=None, size=None, cfg=1),
        "think-edit": dict(mode="gen_text", images=[rgb], bot_task="think", cot=None, size=None, cfg=1),
        "ratio-fusion": dict(mode="gen_text", images=[rgb, rgba], bot_task="img_ratio", cot=None, size=None, cfg=1),
        "image-t2i": dict(mode="gen_image", images=[], bot_task="auto", cot=cot, size="1024x1024", cfg=1),
        "image-t2i-cfg": dict(mode="gen_image", images=[], bot_task="auto", cot=cot, size="1280x768", cfg=2),
        "image-fusion-cfg": dict(mode="gen_image", images=[rgb, rgba], bot_task="auto", cot=cot, size="1152x896", cfg=2),
        "image-edit-nocot": dict(mode="gen_image", images=[rgba], bot_task="auto", cot=None, size="768x1024", cfg=1),
    }
    result = {"vocab_ids": {t: tokenizer.convert_tokens_to_ids(t) for t in ("<boi>", "<eoi>", "<img>", "<think>", "</think>", "<recaption>", "</recaption>")}}
    for name, c in cases.items():
        cond = [processor.build_cond_images(image_list=c["images"], infer_align_image_size=True)] if c["images"] else None
        info = ([processor.build_gen_image_info(c["size"], add_guidance_token=bool(config.cfg_distilled),
                                                add_timestep_r_token=bool(config.use_meanflow))] if c["mode"] == "gen_image" else [None])
        built = tokenizer.apply_chat_template(
            batch_prompt=["A red fox in fresh snow, 图1 and 图2 behind it"], batch_message_list=None, mode=c["mode"],
            batch_gen_image_info=info, batch_cond_images=cond, batch_system_prompt=[system],
            batch_cot_text=[c["cot"]] if c["cot"] else None, max_length=generation["max_length"], bot_task=c["bot_task"],
            image_base_size=processor.vae_reso_group.base_size, sequence_template=generation["sequence_template"],
            cfg_factor=c["cfg"], drop_think=generation["drop_think"],
        )
        result[name] = {"out": plain(dict(built)), "cond": plain(cond), "info": plain(info),
                        "text": tokenizer.decode(built["output"].tokens[0]) if "output" in built else None}
    sizes = {f"{w}:{h}": processor.vae_reso_group.get_target_size(w, h) for w, h in ((1, 1), (16, 9), (9, 16), (4, 3), (21, 9), (1, 4))}
    result["sizes"] = plain(sizes)
    torch.save(result, out)
    print(f"{side}: {len(cases)} cases dumped to {out}")


def compare(a_path: str, b_path: str) -> int:
    a, b = torch.load(a_path, weights_only=False), torch.load(b_path, weights_only=False)
    bad: list[str] = []
    checked = [0]

    def walk(x, y, where: str) -> None:
        checked[0] += 1
        if isinstance(x, torch.Tensor) or isinstance(y, torch.Tensor):
            if not (isinstance(x, torch.Tensor) and isinstance(y, torch.Tensor)):
                bad.append(f"{where}: tensor vs {type(y).__name__}")
            elif x.shape != y.shape or x.dtype != y.dtype:
                bad.append(f"{where}: {tuple(x.shape)} {x.dtype} vs {tuple(y.shape)} {y.dtype}")
            elif x.is_floating_point():
                d = float((x.double() - y.double()).abs().max()) if x.numel() else 0.0
                if d > 0:
                    bad.append(f"{where}: max |diff| {d:.3g} (of max |x| {float(x.double().abs().max()):.3g})")
            elif not torch.equal(x, y):
                n = int((x != y).sum())
                bad.append(f"{where}: {n} of {x.numel()} differ")
        elif isinstance(x, dict) and isinstance(y, dict):
            for k in sorted(set(x) | set(y)):
                if k not in x or k not in y:
                    bad.append(f"{where}.{k}: only in {'a' if k in x else 'b'}")
                else:
                    walk(x[k], y[k], f"{where}.{k}")
        elif isinstance(x, list) and isinstance(y, list):
            if len(x) != len(y):
                bad.append(f"{where}: {len(x)} vs {len(y)} items")
            for i, (p, q) in enumerate(zip(x, y)):
                walk(p, q, f"{where}[{i}]")
        elif x != y:
            bad.append(f"{where}: {str(x)[:120]!r} vs {str(y)[:120]!r}")

    walk(a, b, "")
    for line in bad[:60]:
        print("DIFF", line)
    print(f"{checked[0]} values compared, {len(bad)} differ")
    return 1 if bad else 0


if __name__ == "__main__":
    if sys.argv[1] == "dump":
        dump(Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4], Path(sys.argv[5]))
    else:
        sys.exit(compare(sys.argv[2], sys.argv[3]))
