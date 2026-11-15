"""The TensorD ↔ executor seam's types, compiled from `executor_api` (`tensorfs._ext.executor`)
and typed in `executor.pyi`. The Runtime reads TensorD's commands and answers and writes its
requests through these: an unknown field is ignored, a missing one takes its default, a needed one
raises `Malformed` (or ValueError, building one)."""

from ._ext.executor import (
    COMMANDS,
    Answer,
    Binding,
    ByteGrant,
    CallInterface,
    CallProgress,
    Cancel,
    CheckpointItem,
    ChildCall,
    ChildCancel,
    ChildEvents,
    ChildForget,
    ChildPoll,
    Construction,
    DisplayPace,
    Fork,
    GpuRelease,
    Hello,
    InputFile,
    InputMetadata,
    Invoke,
    Load,
    Malformed,
    Map,
    MemoSettings,
    ModelNeed,
    ModelPrefetch,
    PrepareRequest,
    Preview,
    Publish,
    PublishPart,
    SavedFile,
    StageMemoLookup,
    StageMemoStore,
    Start,
    TensorFSOptions,
    View,
    decode_answer,
    decode_command,
)

#: Any command TensorD sends.
Command = Hello | Start | Load | PrepareRequest | Cancel | Invoke | Map | Fork
#: Any request an executor makes of TensorD.
Request = (
    ChildEvents
    | ChildCall
    | ChildPoll
    | ChildCancel
    | ChildForget
    | GpuRelease
    | ModelPrefetch
    | ModelNeed
    | Publish
    | CheckpointItem
    | StageMemoLookup
    | StageMemoStore
)

__all__ = [
    "COMMANDS",
    "Answer",
    "Binding",
    "ByteGrant",
    "CallInterface",
    "CallProgress",
    "Cancel",
    "CheckpointItem",
    "ChildCall",
    "ChildCancel",
    "ChildEvents",
    "ChildForget",
    "ChildPoll",
    "Command",
    "Construction",
    "DisplayPace",
    "Fork",
    "GpuRelease",
    "Hello",
    "InputFile",
    "InputMetadata",
    "Invoke",
    "Load",
    "Malformed",
    "Map",
    "MemoSettings",
    "ModelNeed",
    "ModelPrefetch",
    "PrepareRequest",
    "Preview",
    "Publish",
    "PublishPart",
    "Request",
    "SavedFile",
    "StageMemoLookup",
    "StageMemoStore",
    "Start",
    "TensorFSOptions",
    "View",
    "decode_answer",
    "decode_command",
]
