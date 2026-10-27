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
app.entrypoint(stages.gpu, name="stages", accelerator=True)
app.entrypoint(repeat.gpu, name="repeat", accelerator=True)
app.entrypoint(model_stages.gpu, name="model_stages", accelerator=True)
app.entrypoint(stored_roles.gpu, name="stored_roles", accelerator=True)
app.entrypoint(source_failure.gpu, name="source_failure", accelerator=True)
app.entrypoint(cold_capture.gpu, name="cold_capture", accelerator=True)
app.entrypoint(shared_pool.gpu, name="shared_pool", accelerator=True)
app.entrypoint(prefetch.gpu, name="prefetch", accelerator=True)
