//! The host-parts constructor's fixed representation; no residency policy.
use super::*;

#[repr(C)]
pub struct HostPart {
    pub data: *const u8,
    pub bytes: usize,
}

fn layout(parts: &[HostPart]) -> Result<(Vec<u64>, usize)> {
    let mut offsets = Vec::with_capacity(parts.len());
    let mut end = 0usize;
    let mut payload = false;
    for part in parts {
        if part.bytes > isize::MAX as usize || (part.bytes != 0 && part.data.is_null()) {
            return Err(invalid("invalid host part"));
        }
        let offset = end
            .checked_add(255)
            .ok_or_else(|| invalid("host parts alignment overflow"))?
            & !255;
        end = offset
            .checked_add(part.bytes)
            .filter(|end| *end <= isize::MAX as usize)
            .ok_or_else(|| invalid("host parts size overflow"))?;
        offsets.push(offset as u64);
        payload |= part.bytes != 0;
    }
    if !payload {
        return Err(invalid("empty host parts aggregate"));
    }
    Ok((offsets, end))
}

/// Every nonempty descriptor must name readable bytes, stable until this returns.
pub(super) unsafe fn snapshot(parts: &[HostPart]) -> Result<(Arc<[u8]>, Vec<u64>)> {
    let (offsets, bytes) = layout(parts)?;
    // Allocate the final Arc once. There is no concatenated Vec or per-part native copy.
    let mut snapshot = Arc::<[u8]>::new_uninit_slice(bytes);
    let destination = Arc::get_mut(&mut snapshot)
        .expect("new unique host snapshot")
        .as_mut_ptr()
        .cast::<u8>();
    // SAFETY: this unique Arc owns exactly `bytes` uninitialized u8 slots. Initialize
    // every slot (including gaps) before assuming initialization. Validated layout
    // keeps all copies in bounds and a new allocation cannot overlap live inputs.
    unsafe {
        std::ptr::write_bytes(destination, 0, bytes);
        for (part, offset) in parts.iter().zip(&offsets) {
            if part.bytes != 0 {
                std::ptr::copy_nonoverlapping(
                    part.data,
                    destination.add(*offset as usize),
                    part.bytes,
                );
            }
        }
        Ok((snapshot.assume_init(), offsets))
    }
}
