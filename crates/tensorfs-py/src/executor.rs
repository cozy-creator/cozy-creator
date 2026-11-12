//! `tensorfs.executor`: the TensorD ↔ executor seam's types (`executor_api`) as Python
//! classes: TensorD's commands and answers, decoded by serde here, and the executor's requests,
//! built and written by serde here. The Runtime keeps no copy of them: an unknown field is
//! ignored, a missing one takes its default, a needed one fails by its path.
use executor_api::{
    Answer, Binding, ByteGrant, CallInterface, CallProgress, Cancel, CheckpointItem, ChildCall, ChildCancel,
    ChildEvents, ChildForget, ChildPoll, Command, Construction, DisplayPace, Fork, GpuRelease, InputFile,
    InputMetadata, Invoke, Load, Map, MemoSettings, ModelNeed, ModelPrefetch, PrepareRequest, Preview, Publish, PublishPart, SavedFile,
    StageMemoLookup, StageMemoStore, Start, TensorFSOptions, View,
};
use pyo3::{create_exception, exceptions::PyValueError, prelude::*, IntoPyObjectExt};

create_exception!(
    tensorfs.executor,
    Malformed,
    PyValueError,
    "A frame of the executor seam that does not decode: its message names the field."
);

/// `{"cmd": "hello"}`: asks the executor who it is.
#[pyclass(module = "tensorfs.executor", name = "Hello", frozen)]
struct HelloCommand;

fn decode<T: serde::de::DeserializeOwned>(data: &[u8]) -> PyResult<T> {
    let mut reader = serde_json::Deserializer::from_slice(data);
    serde_path_to_error::deserialize(&mut reader).map_err(|e| Malformed::new_err(e.to_string()))
}

/// One command TensorD sent, as its class (`Start`, `Load`, ...).
#[pyfunction]
fn decode_command(py: Python<'_>, data: &[u8]) -> PyResult<Py<PyAny>> {
    match Command::decode(data).map_err(Malformed::new_err)? {
        Command::Hello => HelloCommand.into_py_any(py),
        Command::Start(c) => c.into_py_any(py),
        Command::Load(c) => (*c).into_py_any(py),
        Command::PrepareRequest(c) => c.into_py_any(py),
        Command::Cancel(c) => c.into_py_any(py),
        Command::Invoke(c) => (*c).into_py_any(py),
        Command::Map(c) => c.into_py_any(py),
        Command::Fork(c) => c.into_py_any(py),
        Command::Preview(c) => c.into_py_any(py),
    }
}

/// TensorD's answer to one of the executor's requests.
#[pyfunction]
fn decode_answer(data: &[u8]) -> PyResult<Answer> {
    decode(data)
}

pub(crate) fn install(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = parent.py();
    let m = PyModule::new(py, "executor")?;
    m.add_class::<HelloCommand>()?;
    m.add_class::<Start>()?;
    m.add_class::<Load>()?;
    m.add_class::<PrepareRequest>()?;
    m.add_class::<Cancel>()?;
    m.add_class::<Invoke>()?;
    m.add_class::<Map>()?;
    m.add_class::<Fork>()?;
    m.add_class::<Preview>()?;
    m.add_class::<Binding>()?;
    m.add_class::<View>()?;
    m.add_class::<MemoSettings>()?;
    m.add_class::<DisplayPace>()?;
    m.add_class::<Construction>()?;
    m.add_class::<CallInterface>()?;
    m.add_class::<InputMetadata>()?;
    m.add_class::<InputFile>()?;
    m.add_class::<TensorFSOptions>()?;
    m.add_class::<Answer>()?;
    m.add_class::<ByteGrant>()?;
    m.add_class::<CallProgress>()?;
    m.add_class::<ChildEvents>()?;
    m.add_class::<ChildCall>()?;
    m.add_class::<ChildPoll>()?;
    m.add_class::<ChildCancel>()?;
    m.add_class::<ChildForget>()?;
    m.add_class::<GpuRelease>()?;
    m.add_class::<ModelPrefetch>()?;
    m.add_class::<ModelNeed>()?;
    m.add_class::<Publish>()?;
    m.add_class::<PublishPart>()?;
    m.add_class::<SavedFile>()?;
    m.add_class::<CheckpointItem>()?;
    m.add_class::<StageMemoLookup>()?;
    m.add_class::<StageMemoStore>()?;
    m.add("COMMANDS", pyo3::types::PyFrozenSet::new(py, Command::NAMES)?)?;
    m.add("Malformed", py.get_type::<Malformed>())?;
    m.add_function(wrap_pyfunction!(decode_command, &m)?)?;
    m.add_function(wrap_pyfunction!(decode_answer, &m)?)?;
    parent.add_submodule(&m)?;
    py.import("sys")?.getattr("modules")?.set_item("tensorfs._ext.executor", &m)
}
