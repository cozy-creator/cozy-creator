//! The newest of each remote machine's own lines (boot, software, service, GPUs, disk, Hub lease and end; never a run or
//! a project's content), which its Status stream sends as they change: `cozy machine logs <m>` reads them once the
//! machine is gone or doesn't answer (cl-1012). They go with the machine's record.
use api::machine::{MACHINE_LOG_BYTES, MACHINE_LOG_LINES, log_tail};

use crate::machines::Machines;

impl Machines {
    /// Keeps what a frame of the machine's Status says of its own lines. They hold no project's content, so another client's
    /// rental is kept as this controller's are; `local` keeps its own.
    pub(crate) fn keep_log(&self, name: &str, text: &str) {
        if name == "local" {
            return;
        }
        let text = log_tail(text, MACHINE_LOG_LINES, MACHINE_LOG_BYTES);
        if let Err(e) = self.store().keep_machine_log(name, text) {
            tracing::warn!(machine = name, "keeping its log: {e}");
        }
    }

    /// The newest `lines` (0: all) of the machine's own lines kept, if any.
    pub(crate) fn kept_log(&self, name: &str, lines: u64) -> Option<String> {
        let text = self.store().machine_log(name).ok()??;
        Some(log_tail(&text, lines, usize::MAX).to_owned())
    }
}
