use super::*;
mod gpu;
pub(super) fn run(mode: &str, root: &Path, uuid: &str, device: i32) -> Result<Value> {
    gpu::run(mode, root, uuid, device)
}
