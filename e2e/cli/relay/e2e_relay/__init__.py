"""An account's package that calls a root package: `relay` answers with the root stand-in minimax-h3-tools's `echo`."""

import minimax_h3_tools
import msgspec

from cozy_runtime.author import App, call

app = App()


class Relay(msgspec.Struct, forbid_unknown_fields=True):
    text: str


class Relayed(msgspec.Struct):
    text: str
    release: str


async def relay(payload: Relay) -> Relayed:
    echoed = await call(minimax_h3_tools.echo, payload=minimax_h3_tools.Text(text=payload.text))
    return Relayed(text=echoed.text, release=echoed.release)


app.entrypoint(relay, accelerator=False)
