"""Real native memory release gates; no mocks, SDK installation or device emulation."""

from cozy_runtime.author import App

from . import (
    cold_capture,
    model_stages,
    prefetch,
    repeat,
    shared_pool,
    source_failure,
    stages,
    stored_roles,
)

app = App()
for name, module in (
    ("stages", stages),
    ("repeat", repeat),
    ("model_stages", model_stages),
    ("stored_roles", stored_roles),
    ("source_failure", source_failure),
    ("cold_capture", cold_capture),
    ("shared_pool", shared_pool),
    ("prefetch", prefetch),
):
    app.entrypoint(module.gpu, name=name, accelerator=True)
