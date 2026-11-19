# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
"""The cli suite's observed script (xs-220, hack audit runtime #12): one log row of ten fields, past the eight an observation
keeps, logged `times` times."""

import msgspec

from cozy_runtime.author import App, Telemetry

app = App()


class Fields(msgspec.Struct, forbid_unknown_fields=True):
    times: int = 1


class Logged(msgspec.Struct):
    fields: int


@app.entrypoint()
def log(payload: Fields, tel: Telemetry) -> Logged:
    for _ in range(payload.times):
        tel.log("many fields", field_0=0, field_1=1, field_2=2, field_3=3, field_4=4, field_5=5, field_6=6, field_7=7, field_8=8,
                field_9=9)
    return Logged(fields=10)
