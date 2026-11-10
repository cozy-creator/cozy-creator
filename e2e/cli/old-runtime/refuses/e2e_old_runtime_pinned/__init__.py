"""A weightless echo on a lock of Runtime 0.43 (cl-854)."""

import msgspec

from cozy_runtime.author import App

app = App()


class Text(msgspec.Struct, forbid_unknown_fields=True):
    text: str


@app.entrypoint(accelerator=False)
def echo(payload: Text) -> Text:
    return payload
