"""Output quality of one gate, one of analyze.py's CHECKS, with cozy-eval's two cheapest instruments on one CLIP
(ViT-B/16, OpenAI weights, CPU):
  - adherence: cosine similarity between each render (or 3 frames of a video) and what it should show: the cell's caption
    (captions.tsv, its `caption=`), else its prompt (prompts.tsv; gate.sh's `cell` writes both). A prompt that is an
    instruction (an edit of a picture, a question) describes no output: its cell gives a caption and is judged against it,
    or an empty one and is not judged here (the describer's question; "make her shirt red" scored 0.231 on a correct edit
    of a photo). A cell with neither has nothing to be held against (a cutout, an upscale). Every image must clear FLOOR: noise, a flat field, a broken decode or a text encoder
    that ignores the prompt lands far under any real render. A video scores 0.17-0.23 against its long motion prompt
    (every H3 gate since 0.34.5) and noise 0.16-0.18, so no fixed floor separates them: a video is held against the
    last passed gate's score on the same prompt and seed (seeds.tsv; products.sh fixes H3's request), MARGIN under it.
    A mention reads as the prompt of the cell that made its picture (the run's request names each reference's file):
    `<Scene> in bright morning sun` alone names nothing CLIP can see (a correct turbo edit scored 0.228);
  - sharpness: CLIP-IQA's "A sharp image." against "A blurry image.". Single renders vary too much to judge alone (a
    transparent object on grey reads soft), so the gate's median must hold against the last passed gates' medians,
    each taken over the cells both judged (`soft`): a gate of some cells (`--only qwen`) against a passed gate of others
    compares nothing. Six qwen renders of 11-17 scored 0.11-0.54 by the subject (glass, crystal), 0.16 opaque, the
    background they are composited on moving them either way; blurred 6 px, 0.005-0.16.
Never a same-seed pixel diff: a kernel swap moves the pixels (26 dB) without changing the quality.
Measured on the 10-23 gate outputs: adherence 0.325-0.430 for renders against their own prompts, 0.06-0.31 against
another gate's prompts, 0.16-0.18 for noise and grey. Sharpness medians 0.80 for the renders, 0.07 blurred (6 px),
0.04 blocky (1/16), 0.16 noise.
    quality.py <gate dir> [<baseline file>]     the same verdict alone"""
import glob, hashlib, json, os, subprocess, sys, tempfile
from types import ModuleType
from typing import TYPE_CHECKING
import numpy as np
from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runs import Baseline, GateHistory, QualityPass, QualityRef, Run  # noqa: E402

if TYPE_CHECKING:
    from transformers import CLIPModel, CLIPProcessor

MODEL = 'openai/clip-vit-base-patch16'
FLOOR = 0.25
# varena's CPU cell takes 4 Euler steps: a coherent, smeared forest scored 0.239, noise and grey 0.16-0.18.
FLOORS = {'host-sdxl': 0.20}
# A video may score this far under the last passed gate's on the same prompt and seed.
MARGIN = 0.03
# The sharpness median may fall this far under the lowest of the last KEEP passed medians.
BUDGET, KEEP = 0.25, 5
# se-155 (Paul 11-17: Qwen Image 2.1 "soft, with artifacts at 200%"): Qwen-Image-2.1's stock VAE decoder prints a fixed 2-px
# pattern, one pixel of every 2x2 a level or two dark (`dots`), as Diffusers' does; madebyollin's texture-fix decoder, the
# package's default since 0.15.0rc5, cuts it about tenfold. A candidate qwen-image-2's picture whose pattern passes DOTS levels
# fails; a published release's (the `published` suite: 0.14.0 until promotion) is not this gate's to decide.
# Pod-session 44237459562, 170 renders at 2 and 4 MP: stock 0.36-1.01 at the package's settings (0.24 at the lowest, CFG 3),
# texture-fix 0.02-0.11. Known miss: the 0.26.1 gate's qwen-lora (generate_turbo with a LoRA, a dark library) reads 0.177 with the
# stock decoder and passes, its checker visible at 400%; qwen 0.26 and qwen-refs 1.13 fail there (pod-session 44281082970). Tighten
# from the rc5 gate's texture-fix readings.
DOTS = 0.20
_clip: 'tuple[CLIPModel, CLIPProcessor, ModuleType] | None' = None


def clip() -> 'tuple[CLIPModel, CLIPProcessor, ModuleType]':
    global _clip
    if _clip is None:
        import torch
        from transformers import CLIPModel, CLIPProcessor
        torch.set_num_threads(2)
        _clip = (CLIPModel.from_pretrained(MODEL).eval(), CLIPProcessor.from_pretrained(MODEL), torch)  # type: ignore[no-untyped-call]  # transformers' eval() is unannotated
    return _clip


def judge(ims: list[Image.Image], prompt: str) -> list[tuple[float, float]]:
    """(adherence, sharpness) per image."""
    model, proc, torch = clip()
    with torch.no_grad():
        o = model(**proc(text=[prompt, 'A sharp image.', 'A blurry image.'], images=ims, return_tensors='pt', padding=True, truncation=True))
    adherence = (o.image_embeds @ o.text_embeds[:1].T).squeeze(1)
    sharp = o.logits_per_image[:, 1:].softmax(-1)[:, 0]
    return [(round(float(a), 4), round(float(s), 4)) for a, s in zip(adherence, sharp)]


def sources(out: str, names: list[str]) -> tuple[dict[str, str], dict[str, dict[str, str]]]:
    """Which cell made each output file (its digest), and each cell's references (mention -> file digest), from the run
    records: a reference anywhere in a request is `{"key": <mention>, "file": "sha256:..."}`."""
    made: dict[str, str] = {}
    refs: dict[str, dict[str, str]] = {}

    def walk(value: object, into: dict[str, str]) -> None:
        if isinstance(value, dict):
            for ref in value.get('references') or []:
                if isinstance(ref, dict) and isinstance(ref.get('key'), str) and isinstance(ref.get('file'), str):
                    into.setdefault(ref['key'], ref['file'])
            for item in value.values():
                walk(item, into)
        elif isinstance(value, list):
            for item in value:
                walk(item, into)

    for name in names:
        try:
            rec = json.load(open(os.path.join(out, name + '.json')))
        except (OSError, ValueError):
            continue
        for o in rec.get('output') or []:
            if isinstance(o, dict) and o.get('sha256'):
                made.setdefault(o['sha256'], name)
        walk(rec.get('request'), refs.setdefault(name, {}))
    return made, refs


def judged(name: str, prompts: dict[str, str], captions: dict[str, str], made: dict[str, str],
           refs: dict[str, dict[str, str]], seen: tuple[str, ...] = ()) -> str:
    """What `name` is judged against: its caption, else its prompt, each `<mention>` whose picture a cell of this gate made
    reading as what that cell is judged against; any other stays as written, and so does one whose maker's prompt is an
    instruction with no caption."""
    if captions.get(name):
        return captions[name]
    text = prompts[name]
    for key, digest in (refs.get(name) or {}).items():
        maker = made.get(digest)
        if maker is None or maker == name or maker in seen:
            continue
        if captions.get(maker) or (maker in prompts and maker not in captions):
            text = text.replace(f'<{key}>', judged(maker, prompts, captions, made, refs, (*seen, name)))
    return text


def rgb(path: str) -> Image.Image:
    im: Image.Image = Image.open(path)
    if im.mode in ('RGBA', 'LA'):  # transparency: judge the render on mid-grey, not on its alpha
        bg = Image.new('RGBA', im.size, (128, 128, 128, 255)); bg.alpha_composite(im.convert('RGBA')); im = bg
    return im.convert('RGB')


def candidate(target: str) -> str:
    """The package of a run of the gate's packages checkout (gate.sh `pkg` runs `./pkg/fn`, its record says `local/pkg/fn`), as
    ledger.py reads it; '' for a published release's run (`pkg/fn`: the `published` suite runs what users already have)."""
    if not target.startswith(('local/', './')):
        return ''
    parts = [p for p in target.split('@', 1)[0].split('/') if p not in ('local', '_', '.')]
    return parts[-2] if len(parts) >= 2 else ''


def dots(path: str) -> float:
    """The decoder's 2-px pattern, in 8-bit levels: each 2x2 sublattice position's mean of luminance minus its 4x4 box
    average, over the 2x2 blocks that are wholly opaque; their mean |offset|. A flat field or a transparent background
    holds none."""
    im = Image.open(path)
    y = np.asarray(im.convert('L'), dtype=np.float64)
    h, w = y.shape[0] // 2 * 2, y.shape[1] // 2 * 2
    pad = np.pad(y, ((2, 1), (2, 1)), mode='reflect')
    c = np.pad(pad.cumsum(0).cumsum(1), ((1, 0), (1, 0)))
    hp = (y - (c[4:, 4:] - c[:-4, 4:] - c[4:, :-4] + c[:-4, :-4])[: y.shape[0], : y.shape[1]] / 16)[:h, :w]
    keep = np.ones((h // 2, w // 2), dtype=bool)
    if im.mode in ('RGBA', 'LA'):
        a = np.asarray(im.getchannel('A'))[:h, :w] == 255
        keep = a[0::2, 0::2] & a[0::2, 1::2] & a[1::2, 0::2] & a[1::2, 1::2]
    if keep.sum() < 4096:
        return 0.0
    m = np.array([[hp[i::2, j::2][keep].mean() for j in range(2)] for i in range(2)])
    return float(np.abs(m - m.mean()).mean())


def frames(path: str, n: int = 3) -> list[Image.Image]:
    d = tempfile.mkdtemp()
    dur = float(subprocess.run(['ffprobe', '-v', 'error', '-show_entries', 'format=duration', '-of', 'csv=p=0', path],
                               capture_output=True, text=True).stdout.strip() or 0)
    for i in range(n):
        subprocess.run(['ffmpeg', '-v', 'error', '-y', '-ss', f'{dur * (i + 1) / (n + 1):.3f}', '-i', path, '-frames:v', '1', f'{d}/{i}.png'])
    return [Image.open(f).convert('RGB') for f in sorted(glob.glob(f'{d}/*.png'))]


def soft(cells: dict[str, list[tuple[float, float]]], passed: list[QualityPass]) -> tuple[str, bool]:
    """Whether the renders went soft: against each passed gate, the sharpness median over the cells both judged; soft
    when it is BUDGET under every one of theirs. A passed gate that judged none of these cells holds nothing."""
    def median(of: dict[str, list[tuple[float, float]]], names: list[str]) -> float:
        return float(np.median([s for n in names for _, s in of[n]]))
    held = [(h, sorted(set(cells) & set(h.get('cells') or {}))) for h in passed]
    held = [(h, names) for h, names in held if names]
    if not held:
        return (' (no passed gate judged these cells: not held)' if passed else ''), False
    pairs = [(median(cells, names), median(h['cells'], names), len(names)) for h, names in held]
    said = ', '.join(f'{ours:.3f} against {theirs:.3f} on {n} cells' for ours, theirs, n in pairs)
    return f' (against the last passed: {said}; budget {BUDGET})', all(ours < theirs - BUDGET for ours, theirs, _ in pairs)


def check(gate: GateHistory) -> tuple[list[str], list[str]]:
    out, base = gate['out'], gate.get('baseline') or {}
    p = os.path.join(out, 'prompts.tsv')
    prompts = dict(l.rstrip('\n').split('\t', 1) for l in open(p) if '\t' in l) if os.path.exists(p) else {}
    p = os.path.join(out, 'captions.tsv')
    captions = dict(l.rstrip('\n').split('\t', 1) for l in open(p) if '\t' in l) if os.path.exists(p) else {}
    p = os.path.join(out, 'seeds.tsv')
    seeds = dict(l.rstrip('\n').split('\t', 1) for l in open(p) if '\t' in l) if os.path.exists(p) else {}
    label = open(os.path.join(out, 'label')).read().strip()
    kept: dict[str, QualityRef]
    refs, kept = base.get('quality_ref') or {}, {}
    fails: list[str]
    notes: list[str]
    cells: dict[str, list[tuple[float, float]]]
    fails, notes, cells = [], [], {}
    # Every image cell, judged or not: a flat picture is a failed decode (a non-finite one reads back black).
    line: str | None
    for line in open(os.path.join(out, 'cells.tsv')):
        name, _, number = line.rstrip('\n').split('\t')[:3]
        run = Run.read(os.path.join(out, name + '.json')) if number.isdigit() else None
        for f in (run.outputs if run else ()):
            ims = [rgb(f)] if f.endswith(('.png', '.webp', '.jpg')) else frames(f) if f.endswith('.mp4') else []
            flat = [round(float(np.asarray(im, dtype=np.float32).std()), 2) for im in ims]
            if flat and min(flat) < 2.0:
                fails.append(f'pixels {name}: {os.path.basename(f)} is flat (std {min(flat)}): a failed or non-finite decode')
            if run and candidate(run.target) == 'qwen-image-2' and f.endswith(('.png', '.webp')):
                level = dots(f)
                if level > DOTS:
                    fails.append(f'dots {name}: {os.path.basename(f)} carries a 2-px dot grid of {level:.2f} levels, over {DOTS}: '
                                 "Qwen-Image-2.1's stock VAE decoder (se-155)")
                else:
                    notes.append(f'dots {name}: {level:.2f} levels')
    names = [line.split('\t')[0] for line in open(os.path.join(out, 'cells.tsv'))]
    made, mentions = sources(out, names)
    for line in open(os.path.join(out, 'cells.tsv')):
        name, _, number = line.rstrip('\n').split('\t')[:3]
        # Judged against its caption, else its prompt; an empty caption says its prompt is an instruction: not judged.
        if not number.isdigit() or not (captions.get(name) or (name in prompts and name not in captions)):
            continue
        run = Run.read(os.path.join(out, name + '.json'))
        files = run.outputs if run else ()
        ims = [rgb(f) for f in files if f.endswith(('.png', '.webp', '.jpg'))] + [fr for f in files if f.endswith('.mp4') for fr in frames(f)]
        if not ims:
            continue
        text = judged(name, prompts, captions, made, mentions)
        if text != prompts.get(name):
            notes.append(f'quality {name}: judged against "{text}"')
        cells[name] = judge(ims, text)
        worst = min(a for a, _ in cells[name])
        if not any(f.endswith('.mp4') for f in files):
            floor = FLOORS.get(name, FLOOR)
            if worst < floor:
                fails.append(f'quality {name}: adherence {worst:.3f} under {floor}: not what its prompt asked for (noise, flat, broken)')
            continue
        key = hashlib.sha256(f'{prompts.get(name, text)}\t{seeds.get(name, "")}'.encode()).hexdigest()[:16]
        ref = refs.get(name) or {}
        kept[name] = {'key': key, 'adherence': worst, 'label': label}
        if ref.get('key') != key:
            notes.append(f'quality {name}: adherence {worst:.3f}; no passed gate on this prompt and seed to hold it against')
        elif worst < ref['adherence'] - MARGIN:
            fails.append(f'quality {name}: adherence {worst:.3f} under {ref["label"]}\'s {ref["adherence"]:.3f} - {MARGIN} on the same prompt and seed')
        else:
            notes.append(f'quality {name}: adherence {worst:.3f} against {ref["label"]}\'s {ref["adherence"]:.3f} on the same prompt and seed')
    base['quality_ref'] = {**refs, **kept}
    if not cells:
        return fails, notes
    adh = float(np.median([a for v in cells.values() for a, _ in v]))
    med = round(float(np.median([s for v in cells.values() for _, s in v])), 4)
    hist = base.setdefault('quality', [])
    line = f'quality: {len(cells)} cells, adherence median {adh:.3f}, sharpness median {med:.3f}'
    held, failed = soft(cells, hist[-KEEP:])
    line += held
    if failed:
        fails.append(line + ': the renders went soft')
    else:
        notes.append(line)
    hist.append({'sharpness': med, 'adherence': round(adh, 4), 'cells': cells, 'label': label})
    del hist[:-KEEP]
    return fails, notes


if __name__ == '__main__':
    out = sys.argv[1].rstrip('/')
    base: Baseline = json.load(open(sys.argv[2])) if len(sys.argv) > 2 and os.path.exists(sys.argv[2]) else {}
    fails, notes = check({'out': out, 'baseline': base})
    for n in notes: print('note', n)
    for f in fails: print('FAIL', f)
    print('QUALITY', 'FAIL' if fails else 'PASS')
    sys.exit(1 if fails else 0)
