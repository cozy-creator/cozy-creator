# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
"""The held memo cell's tool (cl-1225): weightless, on the CPU, and memoized on its exact input, seed included: a request that gives
no seed is never memoized (its optional `Seed` is its `memo_requires`); the same words with the same seed again on one machine are
answered from the first run's held memo."""

import msgspec
from cozy_runtime.author import App, Seed

app = App()


class Words(msgspec.Struct, kw_only=True):
    text: str
    seed: Seed | None = None


class Counted(msgspec.Struct):
    words: int


@app.entrypoint(accelerator=False, memoize=True)
def count(payload: Words) -> Counted:
    """How many words `text` has."""
    return Counted(words=len(payload.text.split()))
