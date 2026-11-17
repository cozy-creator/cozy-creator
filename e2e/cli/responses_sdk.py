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

from openai import OpenAI


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
    events, deltas, said = [], [], {}
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
        events.append((event.type, event.sequence_number, round(at, 2)))
        if event.type == "response.output_text.delta":
            deltas.append((event.delta, at))
        elif event.type in ("response.output_text.done", "response.completed", "response.failed", "error"):
            said[event.type] = (event, at)
    kinds = [kind for kind, _, _ in events]
    seen["streamed"] = {"events": events[:6] + events[-6:], "deltas": len(deltas)}
    if kinds[:2] != ["response.created", "response.in_progress"]:
        failed.append(f"streamed: it begins {kinds[:2]}, not response.created, response.in_progress")
    for kind in ("response.output_item.added", "response.content_part.added", "response.output_text.done", "response.content_part.done", "response.output_item.done"):
        if kind not in kinds:
            failed.append(f"streamed: no {kind}")
    if not kinds or kinds[-1] != "response.completed":
        failed.append(f"streamed: it ends {kinds[-1:]}, not response.completed: {said.get('response.failed') or said.get('error')}")
    numbers = [number for _, number, _ in events]
    if numbers != list(range(len(numbers))):
        failed.append(f"streamed: sequence numbers {numbers[:12]}…, not 0, 1, 2, …")
    if "response.completed" in said:
        completed, ended = said["response.completed"]
        whole = completed.response.output_text
        seen["streamed"]["text"] = whole
        if "".join(delta for delta, _ in deltas) != whole:
            failed.append(f"streamed: its deltas don't add up to its text: {''.join(delta for delta, _ in deltas)[:200]!r} vs {whole[:200]!r}")
        done = said.get("response.output_text.done")
        if done and done[0].text != whole:
            failed.append("streamed: output_text.done's text isn't the response's")
        if not whole.strip():
            failed.append("streamed: the answer is no words")
        # Streaming, its words come as they are made: several, the first well before it completes.
        if len(deltas) < 3 or deltas[0][1] > ended - 1.0:
            failed.append(f"streamed: {len(deltas)} deltas, the first {deltas[0][1] if deltas else '-'} s and the end {ended:.2f} s: its words came only at its end")

    picture = "data:image/png;base64," + base64.b64encode(png(64, 64)).decode()
    response = client.responses.create(
        model=args.model,
        input=[{"role": "user", "content": [{"type": "input_text", "text": "In one sentence, what colors are in this picture?"}, {"type": "input_image", "image_url": picture}]}],
        max_output_tokens=60,
        temperature=0,
        metadata={"project": args.project},
    )
    seen["whole"] = {"status": response.status, "id": response.id, "text": response.output_text}
    if response.status != "completed" or not response.output_text.strip():
        failed.append(f"not streamed: {response.status}, {response.output_text!r}")

    for line in failed:
        print(f"FAIL {line}")
    print(json.dumps(seen, default=str))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
