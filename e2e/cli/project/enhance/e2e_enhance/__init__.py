"""The cli suite's two prompt enhancers (cl-1101), as Qwen Image 2.1's: `expander_t2i` rewrites a prompt of words and
`expander_i2i` one with pictures, each needed by a rule over its request (cl-1103), and a request reads at most one of them, as its first stage.
`words`, `edit` and `plain` are one function, each with a model and two enhancers of its own, so each request starts with none of
its models here. `behind` is `plain` with both enhancers on demand and no rule: guessed for every request, never read."""

import time

import msgspec

from cozy_runtime.author import App, Loader, Model, Telemetry

app = App()


class Part(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Ask(msgspec.Struct, kw_only=True):
    pictures: bool = False
    prompt_upsampling: bool = True
    seconds: float = 5


class Made(msgspec.Struct):
    #: The enhancer it read, when it started, and how long that read took.
    enhancer: str
    entered_ms: int
    read_ms: int


def _generate(payload: Ask, expander_t2i: Part, model: Part, expander_i2i: Part, tel: Telemetry) -> Made:
    entered, enhancer, read = time.time(), "", 0.0
    if payload.prompt_upsampling:
        with tel.stage("upsampling prompt"):
            enhancer = "expander_i2i" if payload.pictures else "expander_t2i"
            _ = (expander_i2i if payload.pictures else expander_t2i).graph
            read = time.time() - entered
    with tel.stage("generating image"):
        time.sleep(payload.seconds)
    return Made(enhancer=enhancer, entered_ms=int(entered * 1000), read_ms=int(read * 1000))


#: Which requests read each enhancer: one with pictures `expander_i2i`, one without `expander_t2i`, neither with upsampling off.
_NEEDS = {"expander_t2i": "request.prompt_upsampling && !request.pictures",
          "expander_i2i": "request.prompt_upsampling && request.pictures"}


@app.entrypoint(accelerator=False, needs=_NEEDS,
                defaults={"expander_t2i": "e2e/enhance-words-expander-t2i@0.1.0", "model": "e2e/enhance-words-model@0.1.0",
                          "expander_i2i": "e2e/enhance-words-expander-i2i@0.1.0"})  # fmt: skip
def words(payload: Ask, expander_t2i: Part, model: Part, expander_i2i: Part, tel: Telemetry) -> Made:
    return _generate(payload, expander_t2i, model, expander_i2i, tel)


@app.entrypoint(accelerator=False, needs=_NEEDS,
                defaults={"expander_t2i": "e2e/enhance-edit-expander-t2i@0.1.0", "model": "e2e/enhance-edit-model@0.1.0",
                          "expander_i2i": "e2e/enhance-edit-expander-i2i@0.1.0"})  # fmt: skip
def edit(payload: Ask, expander_t2i: Part, model: Part, expander_i2i: Part, tel: Telemetry) -> Made:
    return _generate(payload, expander_t2i, model, expander_i2i, tel)


@app.entrypoint(accelerator=False, needs=_NEEDS,
                defaults={"expander_t2i": "e2e/enhance-plain-expander-t2i@0.1.0", "model": "e2e/enhance-plain-model@0.1.0",
                          "expander_i2i": "e2e/enhance-plain-expander-i2i@0.1.0"})  # fmt: skip
def plain(payload: Ask, expander_t2i: Part, model: Part, expander_i2i: Part, tel: Telemetry) -> Made:
    return _generate(payload, expander_t2i, model, expander_i2i, tel)


@app.entrypoint(accelerator=False, on_demand=("expander_i2i", "expander_t2i"),
                defaults={"expander_t2i": "e2e/enhance-plain-expander-t2i@0.1.0", "model": "e2e/enhance-plain-model@0.1.0",
                          "expander_i2i": "e2e/enhance-plain-expander-i2i@0.1.0"})  # fmt: skip
def behind(payload: Ask, expander_t2i: Part, model: Part, expander_i2i: Part, tel: Telemetry) -> Made:
    return _generate(payload, expander_t2i, model, expander_i2i, tel)
