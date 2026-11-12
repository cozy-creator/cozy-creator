//! Mixed remote logs have no global durable copy. Historical machine-logs remain untouched and
//! unserved until their attributable project records have been preserved and retirement is verified.
use std::sync::Arc;
use api::calcifer::{MachineKind, MachineLog};
use crate::machines::Machines;

impl Machines {
    pub(crate) async fn keep_logs(self: Arc<Self>, _name: String, _kind: MachineKind) {}
    pub(crate) fn kept_log(&self, _name: &str, _log: MachineLog, _lines: u64) -> Option<Vec<u8>> { None }
    pub(crate) fn prune_kept(&self) {}
}
