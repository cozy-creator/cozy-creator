# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.46,<0.47", "msgspec>=0.19"]
# ///
"""gpu-queue's functions (cl-1077): `hold` keeps the pod's one GPU busy, `early` is a plain GPU run, and `late` (on the CPU) calls
`late_call` on the GPU at once, as H3's `edit` calls its segment. Each GPU function builds a model of its own (BiRefNet, a VAE, a
SigLIP), never reading it, so each has its own construction and takes the GPU only for its turn."""

import time

import msgspec

from cozy_runtime.author import App, Loader, Model, Telemetry, call

app = App()


class Extra(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Ask(msgspec.Struct, kw_only=True):
    seconds: float = 0


class Said(msgspec.Struct):
    done_ms: int


@app.entrypoint(accelerator=True, defaults={"extra": "zhengpeng7/birefnet@0.1.0"})
def hold(payload: Ask, extra: Extra, tel: Telemetry) -> Said:
    with tel.stage("holding"):
        time.sleep(payload.seconds)
    return Said(int(time.time() * 1000))


@app.entrypoint(accelerator=True, defaults={"extra": "madebyollin/texture-fix-vae@0.3.0"})
def early(payload: Ask, extra: Extra) -> Said:
    return Said(int(time.time() * 1000))


@app.entrypoint(private=True, accelerator=True, defaults={"extra": "google/siglip2-base-patch16-512@0.1.0"})
def late_call(payload: Ask, extra: Extra) -> Said:
    return Said(int(time.time() * 1000))


@app.entrypoint(accelerator=False)
async def late(payload: Ask) -> Said:
    # The image's Runtime (0.46) names a call's arguments; the RC's stubs know only call(target, request).
    return await call(late_call, payload=Ask())  # type: ignore[call-overload,no-any-return]
