//! Private binding route: ordinary handle leases preserve existing drop custody.
use super::*;

#[doc(hidden)]
pub fn residency_snapshot(
    id: u64,
    region_handles: &[u64],
) -> crate::Result<crate::census::Snapshot> {
    fn error(failure: Failure) -> Error {
        match failure.0 {
            CAPTURING => Error::CaptureActive,
            BUSY => Error::Busy,
            FAULT => Error::Poisoned(failure.1),
            _ => Error::Invalid("invalid residency census handle"),
        }
    }
    let gpu = gpu(id).map_err(error)?;
    let mut links = Vec::with_capacity(region_handles.len());
    {
        // A cloned Weight could become the last owner after a concurrent close;
        // its Region drop may free an unmapped CUDA address. Borrow under the
        // registry lock instead and let only immutable ids leave this scope.
        let registry = registry_unchecked().map_err(error)?;
        for &handle in region_handles {
            let entry = registry
                .get(&handle)
                .filter(|entry| entry.disposition == Disposition::Owned)
                .ok_or(Error::Invalid("invalid residency census Region handle"))?;
            let Handle::Region(weight) = &entry.value else {
                return Err(Error::Invalid("census handle is not a Region"));
            };
            if weight.gpu.device() != gpu.device() {
                return Err(Error::Invalid("census Region belongs to another device"));
            }
            links.push(crate::census::RegionHandle {
                handle,
                arena_id: weight.region.arena_id(),
                region_id: weight.region.id(),
            });
        }
    }
    let mut snapshot = crate::census::snapshot(gpu.device(), gpu.arena.id())?;
    snapshot.region_handles = links;
    Ok(snapshot)
}
