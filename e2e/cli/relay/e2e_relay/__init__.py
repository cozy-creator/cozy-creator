"""An account's package that calls a root package: `relay` answers with the root stand-in minimax-h3-tools's `echo`."""

import msgspec

from cozy_runtime.author import App, call

from .echo_calls import Text, echo

app = App()


class Relay(msgspec.Struct, forbid_unknown_fields=True):
    text: str


class Relayed(msgspec.Struct):
    text: str
    release: str


async def relay(payload: Relay) -> Relayed:
    echoed = await call(echo, Text(text=payload.text))
    return Relayed(text=echoed.text, release=echoed.release)


app.entrypoint(relay, accelerator=False)
