//! Windows: no mount event a process without a window hears.
use crate::{Error, Result, Seam};

pub(super) struct Changes;

impl Changes {
    pub(super) fn new() -> Result<Self> {
        Err(Error::unsupported(Seam::MountWatch))
    }

    pub(super) fn wait(&self) -> Result<()> {
        Ok(())
    }
}
