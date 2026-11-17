"""xs-212 captioning benchmark: one engine's OpenAI-compatible server, streamed chat completions.

python bench.py --url http://127.0.0.1:8000 --label vllm-9b-bf16 --tokenizer /w/models/9b --out /w/out
Phases: story (decode tok/s, ignore_eos), images at each concurrency, videos one at a time.
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import json
import statistics
import time
from pathlib import Path

import aiohttp
from tokenizers import Tokenizer

CAPTION = (
    "Say what this shows, in one or two plain sentences, for someone who can't see it: who or what "
    "it is and how it looks. Say only what you can see."
)
STORY = "Write a long story about a lighthouse keeper who finds a message in a bottle."


def data_url(path: Path, kind: str, salt: int = 0) -> str:
    """The file's bytes; with `salt`, the photo re-encoded (JPEG q95, one pixel nudged) so no engine cache knows it."""
    raw = path.read_bytes()
    if salt:
        import io

        from PIL import Image

        im = Image.open(io.BytesIO(raw)).convert("RGB")
        r, g, b = im.getpixel((0, 0))
        im.putpixel((0, 0), ((r + salt) % 256, g, b))
        out = io.BytesIO()
        im.save(out, "JPEG", quality=95)
        raw = out.getvalue()
    return f"data:{kind};base64,{base64.b64encode(raw).decode()}"


async def one(session, url, body, tok, rid):
    t0 = time.perf_counter()
    first = None
    text = []
    usage = None
    async with session.post(f"{url}/v1/chat/completions", json=body) as r:
        if r.status != 200:
            return {"id": rid, "error": f"{r.status} {(await r.text())[:500]}"}
        buf = b""
        async for chunk in r.content.iter_any():
            buf += chunk
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                line = line.strip()
                if not line.startswith(b"data:"):
                    continue
                data = line[5:].strip()
                if data == b"[DONE]":
                    continue
                ev = json.loads(data)
                if ev.get("usage"):
                    usage = ev["usage"]
                for c in ev.get("choices") or []:
                    d = (c.get("delta") or {}).get("content")
                    if d:
                        if first is None:
                            first = time.perf_counter()
                        text.append(d)
    end = time.perf_counter()
    out = "".join(text)
    n = (usage or {}).get("completion_tokens") or len(tok.encode(out, add_special_tokens=False).ids)
    return {
        "id": rid,
        "ttft": (first or end) - t0,
        "total": end - t0,
        "tokens": n,
        "prompt_tokens": (usage or {}).get("prompt_tokens"),
        "decode_tps": (n - 1) / (end - first) if first and n > 1 and end > first else None,
        "text": out,
    }


def body(content, max_tokens, extra):
    b = {
        "model": "m",
        "messages": [{"role": "user", "content": content}],
        "max_tokens": max_tokens,
        "temperature": 0,
        "stream": True,
        "stream_options": {"include_usage": True},
        "chat_template_kwargs": {"enable_thinking": False},
    }
    b.update(extra)
    return b


async def phase(url, bodies, conc, tok, names=None):
    sem = asyncio.Semaphore(conc)
    timeout = aiohttp.ClientTimeout(total=3600)
    async with aiohttp.ClientSession(timeout=timeout) as s:

        async def go(i, b):
            async with sem:
                return await one(s, url, b, tok, i)

        t0 = time.perf_counter()
        rows = await asyncio.gather(*(go(i, b) for i, b in enumerate(bodies)))
        wall = time.perf_counter() - t0
    for r, name in zip(rows, names or []):
        r["item"] = name
    return wall, rows


def summary(name, wall, rows):
    ok = [r for r in rows if "error" not in r]
    toks = sum(r["tokens"] for r in ok)
    s = {
        "phase": name,
        "n": len(rows),
        "errors": len(rows) - len(ok),
        "wall_s": round(wall, 3),
        "out_tok": toks,
        "out_tok_s": round(toks / wall, 1) if wall else None,
        "req_s": round(len(ok) / wall, 3) if wall else None,
    }
    if ok:
        s["ttft_mean_s"] = round(statistics.mean(r["ttft"] for r in ok), 3)
        s["ttft_p50_s"] = round(statistics.median(r["ttft"] for r in ok), 3)
        s["lat_p50_s"] = round(statistics.median(r["total"] for r in ok), 3)
        d = [r["decode_tps"] for r in ok if r["decode_tps"]]
        s["decode_tps_mean"] = round(statistics.mean(d), 1) if d else None
        p = [r["prompt_tokens"] for r in ok if r["prompt_tokens"]]
        s["prompt_tok_mean"] = round(statistics.mean(p)) if p else None
    return s


async def main():
    a = argparse.ArgumentParser()
    a.add_argument("--url", required=True)
    a.add_argument("--label", required=True)
    a.add_argument("--tokenizer", required=True)
    a.add_argument("--out", required=True)
    a.add_argument("--images", default="/w/data/images")
    a.add_argument("--videos", default="/w/data/videos")
    a.add_argument("--conc", default="1,8,32")
    a.add_argument("--phases", default="story,images,videos")
    a.add_argument("--max-tokens", type=int, default=256)
    a.add_argument("--extra", default="{}", help="JSON merged into every request body")
    a.add_argument("--video-as", default="video_url", help="video_url or none")
    o = a.parse_args()
    tok = Tokenizer.from_file(str(Path(o.tokenizer) / "tokenizer.json"))
    extra = json.loads(o.extra)
    out = Path(o.out)
    out.mkdir(parents=True, exist_ok=True)
    phases = o.phases.split(",")
    sums = []
    rows_all = {}
    # warm-up: one image, not counted (graph capture, first-call compiles)
    imgs = sorted(Path(o.images).glob("*.jpg"))
    warm = body(
        [{"type": "image_url", "image_url": {"url": data_url(imgs[-1], "image/jpeg", 99)}}, {"type": "text", "text": CAPTION}],
        o.max_tokens,
        extra,
    )
    w, r = await phase(o.url, [warm], 1, tok)
    sums.append(summary("warmup", w, r))
    print(json.dumps(sums[-1]), flush=True)
    if "story" in phases:
        b = body(STORY, 512, {**extra, "ignore_eos": True})
        w, r = await phase(o.url, [b], 1, tok)
        sums.append(summary("story", w, r))
        rows_all["story"] = r
        print(json.dumps(sums[-1]), flush=True)
    if "images" in phases:
        for k, c in enumerate(int(x) for x in o.conc.split(",")):
            bodies = [
                body(
                    [{"type": "image_url", "image_url": {"url": data_url(p, "image/jpeg", k)}}, {"type": "text", "text": CAPTION}],
                    o.max_tokens,
                    extra,
                )
                for p in imgs[:32]
            ]
            w, r = await phase(o.url, bodies, c, tok, [p.stem for p in imgs[:32]])
            sums.append(summary(f"images_c{c}", w, r))
            rows_all[f"images_c{c}"] = r
            print(json.dumps(sums[-1]), flush=True)
    if "videos" in phases and o.video_as != "none":
        vids = sorted(Path(o.videos).glob("*.mp4"))
        bodies = [
            body(
                [{"type": "video_url", "video_url": {"url": data_url(p, "video/mp4")}}, {"type": "text", "text": CAPTION}],
                o.max_tokens,
                extra,
            )
            for p in vids
        ]
        w, r = await phase(o.url, bodies, 1, tok, [p.stem for p in vids])
        sums.append(summary("videos_c1", w, r))
        rows_all["videos_c1"] = r
        print(json.dumps(sums[-1]), flush=True)
    (out / f"{o.label}.summary.json").write_text(json.dumps(sums, indent=1))
    (out / f"{o.label}.rows.json").write_text(json.dumps(rows_all, indent=1))


if __name__ == "__main__":
    asyncio.run(main())
