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

/// Host ranges a Region fills from without copying them. They stay readable and unchanged until
/// `drop`, which runs once on whichever thread lets go last, in-flight fills included.
pub(super) struct Borrowed {
    parts: Vec<(usize, usize, usize)>,
    bytes: usize,
    drop: DropFn,
    context: *mut c_void,
}
// SAFETY: the caller's contract above: immutable bytes, and a drop callable from any thread.
unsafe impl Send for Borrowed {}
unsafe impl Sync for Borrowed {}

impl Drop for Borrowed {
    fn drop(&mut self) {
        // SAFETY: called exactly once, as the caller's contract asks.
        unsafe { (self.drop)(self.context) }
    }
}

impl Borrowed {
    /// Takes custody of `context` even on failure, so the caller never has two paths.
    pub(super) unsafe fn new(parts: &[HostPart], drop: DropFn, context: *mut c_void) -> Result<(Arc<Self>, Vec<u64>)> {
        let mut borrowed = Self { parts: Vec::with_capacity(parts.len()), bytes: 0, drop, context };
        let (offsets, bytes) = layout(parts)?;
        borrowed.bytes = bytes;
        for (part, offset) in parts.iter().zip(&offsets) {
            if part.bytes != 0 {
                borrowed.parts.push((*offset as usize, part.data as usize, part.bytes));
            }
        }
        Ok((Arc::new(borrowed), offsets))
    }

    pub(super) fn len(&self) -> usize {
        self.bytes
    }

    /// Copy what lies in `[at, at + len)` to `to`. Gaps stay as they were: nothing reads them.
    pub(super) unsafe fn read(&self, at: usize, to: *mut u8, len: usize) {
        let first = self.parts.partition_point(|(offset, _, bytes)| offset + bytes <= at);
        for &(offset, address, bytes) in &self.parts[first..] {
            if offset >= at + len {
                break;
            }
            let (from, end) = (offset.max(at), (offset + bytes).min(at + len));
            // SAFETY: the caller's contract keeps the part readable; `to` holds `len` bytes.
            unsafe {
                std::ptr::copy_nonoverlapping((address + from - offset) as *const u8, to.add(from - at), end - from);
            }
        }
    }
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
