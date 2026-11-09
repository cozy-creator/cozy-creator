"""Types of `tensorfs.executor`: one class per struct of the `executor-api` crate, field for field."""

from pathlib import Path
from typing import Any, Literal, TypeAlias

from typing_extensions import Self

#: Every `cmd` TensorD sends.
COMMANDS: frozenset[str]

class Malformed(ValueError): ...

class _Wire:
    def json(self) -> str: ...
    def replace(self, **changes: Any) -> Self: ...

# ------------------------------------------------------------------ TensorD's commands

class Hello: ...

class TensorFSOptions:
    endpoint: str
    token: str
    expires_unix_ms: int
    ca_der: bytes

class MemoSettings(_Wire):
    process_bytes: int
    entry_bytes: int
    def __init__(self, *, process_bytes: int = -1, entry_bytes: int = 0) -> None: ...

class DisplayPace:
    work_ms: int
    pause_ms: int

class CallInterface(_Wire):
    module: str
    export: str
    interface_path: Path
    self_call: bool
    interface_document: Any | None
    builtin: str | None
    #: The wire spells `self_call` as `self`.
    def __init__(self, **fields: Any) -> None: ...

class Construction(_Wire):
    path: str
    manifest: str
    components: list[str]
    def __init__(self, *, path: str, manifest: str, components: list[str] = ...) -> None: ...

class Start(_Wire):
    devices: str
    application: str
    package_interface: Path
    sequence_parallel_degree: int
    import_only: bool
    call_interfaces: list[CallInterface]
    constructions: list[Construction]
    memo: MemoSettings | None
    gpu_memory_limit: int | None
    shared: bool
    display_pace: DisplayPace | None
    def __init__(self, **fields: Any) -> None: ...

class View:
    #: The cozytensors header, base64.
    header: str
    #: The objects it cites that no store holds, base64.
    own: list[str]

class Binding(_Wire):
    application: str
    package_interface: str
    model_class: str
    model_binding_path: str
    model_parameter_name: str
    components: list[str]
    store: str
    snapshot: str
    release: str
    package: str
    model: str
    view: View | None
    homes: dict[str, int]
    def __init__(self, **fields: Any) -> None: ...

class Load(_Wire):
    construction: str
    tensorfs: TensorFSOptions | None
    actor: str
    devices: str
    sequence_parallel_degree: int
    models: list[Binding]
    attention_pin: str
    def __init__(self, **fields: Any) -> None: ...

class InputMetadata(_Wire):
    input_id: str
    media_type: str
    digest: str
    length: int
    order: int
    def __init__(self, **fields: Any) -> None: ...

class InputFile(_Wire):
    local: Path
    media_type: str
    digest: str
    length: int
    order: int
    def __init__(self, **fields: Any) -> None: ...

class PrepareRequest(_Wire):
    request_id: str
    construction: str
    entrypoint: str
    payload: Any
    attention_kernel: str
    bits: int
    input_metadata: dict[str, InputMetadata]
    def __init__(self, **fields: Any) -> None: ...

class Cancel(_Wire):
    request_id: str
    def __init__(self, *, request_id: str) -> None: ...

class Invoke(_Wire):
    tensorfs: TensorFSOptions | None
    max_output_bytes: int
    request_id: str
    construction: str
    entrypoint: str
    spool: Path
    deadline_s: float | None
    attention_kernel: str
    bits: int
    inputs: dict[str, InputFile]
    trees: dict[str, tuple[Path, str]]
    sources: dict[str, str]
    weights: dict[str, int]
    destination: str
    actor: str
    operation: str
    family: str
    def __init__(self, **fields: Any) -> None: ...

class Map(_Wire):
    construction: str
    level: Literal["gpu", "ram"]
    request_id: str
    entrypoint: str
    payload: Any | None
    def __init__(self, **fields: Any) -> None: ...

class Fork(_Wire):
    socket: Path
    root: Path
    environment: dict[str, str]
    def __init__(self, **fields: Any) -> None: ...

Command: TypeAlias = Hello | Start | Load | PrepareRequest | Cancel | Invoke | Map | Fork

def decode_command(data: bytes) -> Command: ...

# ------------------------------------------------------------------ TensorD's answers

class ByteGrant(_Wire):
    output_id: str
    kind: str
    digest: str
    local: Path
    length: int
    content_bytes: int
    media_type: str
    def __init__(self, **fields: Any) -> None: ...

class CallProgress(_Wire):
    sequence: int
    payload: Any
    def __init__(self, *, sequence: int, payload: Any) -> None: ...

class Answer(_Wire):
    event: str
    seq: int
    ok: bool
    code: str
    detail: str
    descriptor: bool
    sha256: str
    length: int
    digest: str
    sequence: int
    state: str
    child_request_id: str
    result: str
    byte_grants: list[ByteGrant]
    progress: CallProgress | None
    traceback: str
    local: str
    cost_ms: float
    disputed: bool
    stored: bool
    reason: str
    def __init__(self, **fields: Any) -> None: ...

def decode_answer(data: bytes) -> Answer: ...

# ------------------------------------------------------------------ the executor's requests

class ChildEvents(_Wire):
    def __init__(self) -> None: ...

class SavedFile(_Wire):
    asset_ref: str
    digest: str
    media_type: str
    def __init__(self, *, asset_ref: str, digest: str, media_type: str = "") -> None: ...

class ChildCall(_Wire):
    call_index: int
    module: str
    export: str
    payload: str
    progress_label: str
    saved: list[SavedFile]
    def __init__(
        self,
        *,
        call_index: int,
        module: str,
        export: str,
        payload: str,
        progress_label: str = "",
        saved: list[SavedFile] | tuple[SavedFile, ...] = (),
    ) -> None: ...

class ChildPoll(_Wire):
    call_index: int
    def __init__(self, *, call_index: int) -> None: ...

class ChildCancel(_Wire):
    call_index: int
    def __init__(self, *, call_index: int) -> None: ...

class ChildForget(_Wire):
    call_index: int
    def __init__(self, *, call_index: int) -> None: ...

class GpuRelease(_Wire):
    def __init__(self) -> None: ...

class ModelPrefetch(_Wire):
    module: str
    export: str
    payload: str
    def __init__(self, *, module: str, export: str, payload: str = "") -> None: ...

class PublishPart(_Wire):
    local: str
    duration_us: int
    def __init__(self, *, local: str, duration_us: int = 0) -> None: ...

class Publish(_Wire):
    output: str
    label: str
    asset_ref: str
    asset_kind: str
    media_type: str
    size_bytes: int
    digest: str
    parts: list[PublishPart]
    def __init__(
        self,
        *,
        output: str,
        asset_ref: str,
        asset_kind: str,
        label: str = "",
        media_type: str = "",
        size_bytes: int = 0,
        digest: str = "",
        parts: list[PublishPart] | tuple[PublishPart, ...] = (),
    ) -> None: ...

class CheckpointItem(_Wire):
    output: str
    repo_id: str
    revision: str
    tag: str
    variant: str
    def __init__(
        self, *, output: str, repo_id: str, revision: str, tag: str = "", variant: str = ""
    ) -> None: ...

class StageMemoLookup(_Wire):
    key: str
    stage: str
    numerics: str
    def __init__(self, *, key: str, stage: str, numerics: str) -> None: ...

class StageMemoStore(_Wire):
    key: str
    stage: str
    numerics: str
    local: str
    sha256: str
    length: int
    cost_ms: float
    reason: str
    def __init__(
        self,
        *,
        key: str,
        stage: str,
        numerics: str,
        local: str = "",
        sha256: str = "",
        length: int = 0,
        cost_ms: float = 0.0,
        reason: str = "",
    ) -> None: ...

Request: TypeAlias = (
    ChildEvents | ChildCall | ChildPoll | ChildCancel | ChildForget | GpuRelease | ModelPrefetch
    | Publish | CheckpointItem | StageMemoLookup | StageMemoStore
)
