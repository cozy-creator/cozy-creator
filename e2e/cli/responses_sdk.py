"""xs-209: OpenAI's own Python SDK against Calcifer's Responses API (POST /v1/responses), with the CLI's bearer as its API key.

Streamed, the answer comes as Responses events in their order, its words as deltas well before it completes, which add up to
its text; not streamed, a question about a picture sent as a data: URL answers as the Response whole. Prints one FAIL line a
failed check, then what it saw as JSON."""

import argparse
import base64
import json
import sys
import time
import zlib

from openai import OpenAI  # type: ignore[import-not-found]  # the cell runs it with `uv run --with openai`; the type check's environment has none


def png(width: int, height: int) -> bytes:
    """A red square on blue, as PNG."""
    rows = b"".join(
        b"\x00" + b"".join(b"\xd0\x20\x20" if abs(x - width / 2) < width / 4 and abs(y - height / 2) < height / 4 else b"\x20\x40\xd0" for x in range(width))
        for y in range(height)
    )

    def chunk(kind: bytes, data: bytes) -> bytes:
        return len(data).to_bytes(4, "big") + kind + data + zlib.crc32(kind + data).to_bytes(4, "big")

    header = width.to_bytes(4, "big") + height.to_bytes(4, "big") + b"\x08\x02\x00\x00\x00"
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


def main() -> int:
    asked = argparse.ArgumentParser()
    for name in ("base", "key", "project", "model"):
        asked.add_argument(f"--{name}", required=True)
    args = asked.parse_args()
    client = OpenAI(base_url=args.base, api_key=args.key, max_retries=0, timeout=900)
    failed: list[str] = []
    seen: dict[str, object] = {}

    began = time.monotonic()
    events: list[tuple[str, int, float]] = []
    deltas: list[tuple[str, float]] = []
    whole: str | None = None
    done: str | None = None
    ended = 0.0
    failure = ""
    stream = client.responses.create(
        model=args.model,
        input="Write six sentences about a lighthouse at night.",
        instructions="Answer in plain English.",
        max_output_tokens=200,
        temperature=0,
        metadata={"project": args.project},
        stream=True,
    )
    for event in stream:
        at = time.monotonic() - began
        kind = str(event.type)
        events.append((kind, int(event.sequence_number), round(at, 2)))
        if kind == "response.output_text.delta":
            deltas.append((str(event.delta), at))
        elif kind == "response.output_text.done":
            done = str(event.text)
        elif kind == "response.completed":
            whole, ended = str(event.response.output_text), at
        elif kind in ("response.failed", "error"):
            failure = str(event)[:500]
    kinds = [kind for kind, _, _ in events]
    streamed: dict[str, object] = {"events": events[:6] + events[-6:], "deltas": len(deltas), "text": whole}
    seen["streamed"] = streamed
    if kinds[:2] != ["response.created", "response.in_progress"]:
        failed.append(f"streamed: it begins {kinds[:2]}, not response.created, response.in_progress")
    for kind in ("response.output_item.added", "response.content_part.added", "response.output_text.done", "response.content_part.done", "response.output_item.done"):
        if kind not in kinds:
            failed.append(f"streamed: no {kind}")
    if not kinds or kinds[-1] != "response.completed":
        failed.append(f"streamed: it ends {kinds[-1:]}, not response.completed: {failure}")
    numbers = [number for _, number, _ in events]
    if numbers != list(range(len(numbers))):
        failed.append(f"streamed: sequence numbers {numbers[:12]}…, not 0, 1, 2, …")
    if whole is not None:
        joined = "".join(delta for delta, _ in deltas)
        if joined != whole:
            failed.append(f"streamed: its deltas don't add up to its text: {joined[:200]!r} vs {whole[:200]!r}")
        if done is not None and done != whole:
            failed.append("streamed: output_text.done's text isn't the response's")
        if not whole.strip():
            failed.append("streamed: the answer is no words")
        # Streaming, its words come as they are made: several, the first well before it completes.
        if len(deltas) < 3 or deltas[0][1] > ended - 1.0:
            first = f"{deltas[0][1]:.2f}" if deltas else "-"
            failed.append(f"streamed: {len(deltas)} deltas, the first at {first} s and the end at {ended:.2f} s: its words came only at its end")

    picture = "data:image/png;base64," + base64.b64encode(png(256, 256)).decode()
    response = client.responses.create(
        model=args.model,
        input=[{"role": "user", "content": [{"type": "input_text", "text": "In one sentence, what colors are in this picture?"}, {"type": "input_image", "image_url": picture}]}],
        max_output_tokens=60,
        temperature=0,
        metadata={"project": args.project},
    )
    status, text, error = str(response.status), str(response.output_text), str(response.error)
    seen["whole"] = {"status": status, "id": str(response.id), "text": text, "error": error}
    if status != "completed" or not text.strip():
        failed.append(f"not streamed: {status}, {text!r}: {error}")

    for line in failed:
        print(f"FAIL {line}")
    print(json.dumps(seen, default=str))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
