"""Public software fixture. Private subjects enter only as later runtime requests."""
import msgspec

from cozy_runtime.author import App, Outputs, Telemetry
from cozy_runtime.media import File

app = App()


class Request(msgspec.Struct, forbid_unknown_fields=True):
    source: File
    text: str
    fail: bool = False


class Answer(msgspec.Struct):
    file: File
    text: str


@app.entrypoint(accelerator=False)
def echo(payload: Request, out: Outputs, tel: Telemetry) -> Answer:
    content = payload.source.read_bytes() + b"\n" + payload.text.encode()
    print(payload.text, flush=True)
    # stdout is a raw executor diagnostic in the released cohort. The supported
    # telemetry service creates its actual durable worker/controller run log.
    tel.log(payload.text, level="info")
    if payload.fail:
        raise RuntimeError(payload.text)
    target = out.temporary_file(".txt")
    target.write_bytes(content)
    return Answer(out.save_file(target, media_type="text/plain"), payload.text)
