"""A Hub outage between the pieces of a run (cl-708): a deviceless root calls `piece` once per model, pausing before
each call after the first, so a session can take the Hub away between them. Each piece's model resolves before it
runs: while the Hub is unreachable the call waits and its run says it retries; a model an earlier piece resolved asks
the Hub nothing; a model the Hub does not have fails its call at once, which the root reports instead of raising."""

from __future__ import annotations

import asyncio
import time

import msgspec

from cozy_runtime.author import App, ChildCallError, Context, ModelChoice, Weights, invocable

app = App()


class Read(msgspec.Struct):
    checkpoint: str


@invocable
async def piece(ctx: Context, *, model: Weights) -> Read:
    """The checkpoint its model resolved to."""
    return Read(model.checkpoint_ref)


app.child_task(piece, accelerator=False)


class Pieces(msgspec.Struct):
    #: One call per model (`org/name[@release]`), in order.
    models: list[str]
    #: Seconds before each call after the first.
    pause_s: float = 20.0


class Piece(msgspec.Struct):
    model: str
    seconds: float
    checkpoint: str = ""
    #: The call's refusal, when it was refused.
    code: str = ""


class Report(msgspec.Struct):
    pieces: list[Piece]


async def pieces(ctx: Context, payload: Pieces) -> Report:
    done = []
    for i, ref in enumerate(payload.models):
        if i:
            await asyncio.sleep(payload.pause_s)
        started = time.monotonic()
        try:
            read = await piece(model=ModelChoice(ref=ref))
            done.append(Piece(ref, round(time.monotonic() - started, 2), checkpoint=read.checkpoint))
        except ChildCallError as refused:
            done.append(Piece(ref, round(time.monotonic() - started, 2), code=refused.code))
    return Report(done)


app.entrypoint(pieces, accelerator=False)
