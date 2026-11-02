// Negative control appended only in the CPU release-gate copy: deliberately omit the
// product tombstone to demonstrate that renaming alone does not reject old lock waiters.
impl Prepared {
    pub(super) fn gate_rename_only(self) -> Result<()> {
        fs::rename(&self.temporary, &self.path)?;
        Ok(())
    }
}
