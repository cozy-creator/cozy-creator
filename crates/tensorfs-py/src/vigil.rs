//! `tensorfs.executor.vigil` (cl-977): the executor's own reading of its threads for TensorD's hang watch.
use pyo3::prelude::*;

/// This process's vigil (cl-977), on `fd` (its end of the line TensorD reads; this takes it): a native thread, `~vigil`, that
/// never takes the GIL, answers each ask (a line with its number) with this process's own threads (`platform::process::Reading`):
/// what TensorD's hang watch can't read itself where the host denies it ptrace. It ends when the line does.
#[cfg(target_os = "linux")]
#[pyfunction]
pub(crate) fn vigil(fd: i32) -> PyResult<()> {
    use std::{
        io::{BufRead, BufReader, Write},
        os::{fd::FromRawFd, unix::net::UnixStream},
    };
    // SAFETY: the caller hands this descriptor over (`socket.detach()`), and nothing else closes it.
    let line = unsafe { UnixStream::from_raw_fd(fd) };
    let mut answers = line.try_clone()?;
    std::thread::Builder::new().name("~vigil".into()).spawn(move || {
        for asked in BufReader::new(line).lines() {
            let Ok(ask) = asked.map(|ask| ask.trim().parse::<u64>()) else { return };
            let Ok(ask) = ask else { continue };
            let threads = platform::process::lives(std::process::id()).unwrap_or_default();
            let Ok(mut answer) = serde_json::to_vec(&platform::process::Reading { ask, threads }) else { continue };
            answer.push(b'\n');
            if answers.write_all(&answer).is_err() {
                return;
            }
        }
    })?;
    Ok(())
}
