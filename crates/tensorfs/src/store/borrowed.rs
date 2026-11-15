//! Read-only sharing of an already-authorized source store. Liveness uses its existing GC
//! gate, never a source-store hold/use record containing the caller's private derived identity.
use std::{collections::BTreeMap, fs::File, path::PathBuf, sync::Arc};

use super::{invalid, retention, Error, ObjectRef, Result, Store};

#[derive(Clone, Debug)]
pub struct BorrowedObjects(Arc<ReadOnly>);

#[derive(Debug)]
struct ReadOnly {
    store: Store,
    objects: BTreeMap<String, ObjectRef>,
    // One descriptor per borrowed source, not one descriptor per model chunk.
    _fence: Option<retention::Fence>,
}

impl Store {
    /// Borrow only objects the caller has already authorized as public. Opens are read-only
    /// and lazy; this writes no uses, holds, refs or caller identity into the source store.
    pub fn borrow_objects(&self, objects: &[ObjectRef]) -> Result<BorrowedObjects> {
        let fence = (!objects.is_empty()).then(|| retention::acquire(self)).transpose()?;
        let mut selected = BTreeMap::new();
        for object in objects {
            if let Some(old) = selected.insert(object.sha256.clone(), object.clone()) {
                if old != *object {
                    return Err(invalid(self.root(), "borrowed object has conflicting lengths"));
                }
            }
            // Validate admission and length without touching its use metadata. This temporary
            // descriptor closes immediately; the shared GC gate protects later lazy opens.
            drop(self.file(object)?);
        }
        Ok(BorrowedObjects(Arc::new(ReadOnly { store: self.clone(), objects: selected, _fence: fence })))
    }
}

impl BorrowedObjects {
    pub(super) fn holds(&self, object: &ObjectRef) -> bool {
        self.0.objects.get(&object.sha256) == Some(object)
    }

    pub(super) fn file(&self, object: &ObjectRef) -> Result<File> {
        if !self.holds(object) { return Err(Error::Absent(object.sha256.clone())); }
        self.0.store.file(object)
    }

    pub(super) fn path(&self, object: &ObjectRef) -> Option<PathBuf> {
        self.holds(object).then(|| self.0.store.object_path(&object.sha256))
    }
}
