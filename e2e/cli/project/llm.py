# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.45", "msgspec>=0.19"]
# ///
"""xs-204's LLM, in the shape of cozy_runtime.vlm's box: a prompt that reads pictures and videos, a seed, and an answer in words. It
answers with what it was asked and what it was shown, so a cell reads that a box's words were its prompt and its mention the picture."""

import msgspec
from cozy_runtime.author import App, Seed
from cozy_runtime.fields import Image, Prompt, Video

app = App()


class Asked(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image | Video] = Prompt()
    seed: Seed | None = None


class Answer(msgspec.Struct):
    text: str


@app.tool(label="Ask", named="answer")
def ask(payload: Asked) -> Answer:
    """Answers in words, from words and what they show."""
    shown = [f"an image {ref.file.probe().width}px wide" if isinstance(ref, Image) else "a video" for ref in payload.instructions.references.values()]
    return Answer(f"ANSWER to: {payload.instructions.text.strip()} | shown: {', '.join(shown) or 'nothing'} | seed {payload.seed}")
