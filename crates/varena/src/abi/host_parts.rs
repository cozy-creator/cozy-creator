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

#[cfg(test)]
mod tests {
    use super::*;

    fn part(bytes: &[u8]) -> HostPart {
        HostPart {
            data: bytes.as_ptr(),
            bytes: bytes.len(),
        }
    }

    #[test]
    fn snapshot_alignment_gaps_stage_crossing_and_immutability() {
        let mut first = vec![0x17; (2 << 20) - 1];
        let second = [0x39; 257];
        let parts = [part(&first), part(&second), part(&[]), part(&[0x5b])];
        let (snapshot, offsets) = unsafe { snapshot(&parts) }.unwrap();
        assert_eq!(offsets, [0, 2 << 20, (2 << 20) + 512, (2 << 20) + 512]);
        first.fill(0);
        assert!(snapshot[..first.len()].iter().all(|byte| *byte == 0x17));
        assert_eq!(snapshot[first.len()], 0);
        assert_eq!(
            &snapshot[offsets[1] as usize..offsets[1] as usize + 257],
            &second
        );
        assert!(
            snapshot[offsets[1] as usize + 257..offsets[3] as usize]
                .iter()
                .all(|byte| *byte == 0)
        );
        assert_eq!(snapshot.last(), Some(&0x5b));
    }

    #[test]
    fn empty_members_are_aligned_but_empty_aggregate_is_invalid() {
        let empty = HostPart {
            data: std::ptr::null(),
            bytes: 0,
        };
        assert!(layout(&[]).is_err());
        assert!(layout(&[empty]).is_err());
        let (snapshot, offsets) = unsafe { snapshot(&[part(&[1]), part(&[])]) }.unwrap();
        assert_eq!(offsets, [0, 256]);
        assert_eq!(snapshot.len(), 256);
        assert!(snapshot[1..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn invalid_and_overflowing_descriptors_refuse_before_copy() {
        let invalid = HostPart {
            data: std::ptr::null(),
            bytes: 1,
        };
        assert!(unsafe { snapshot(&[invalid]) }.is_err());
        let huge = HostPart {
            data: std::ptr::dangling(),
            bytes: usize::MAX,
        };
        assert!(unsafe { snapshot(&[huge]) }.is_err());
        let huge = HostPart {
            data: std::ptr::dangling(),
            bytes: isize::MAX as usize,
        };
        assert!(unsafe { snapshot(&[huge, part(&[1])]) }.is_err());
    }

    #[test]
    fn failed_factory_leaves_outputs_unchanged() {
        let mut offsets = [123u64];
        let mut handle = 456;
        assert_eq!(
            unsafe {
                vr_v1_region_from_host_parts(
                    0,
                    std::ptr::null(),
                    1,
                    offsets.as_mut_ptr(),
                    &mut handle,
                )
            },
            INVALID
        );
        assert_eq!(offsets, [123]);
        assert_eq!(handle, 456);
        assert_eq!(
            unsafe {
                vr_v1_region_from_host_parts(
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null_mut(),
                    &mut handle,
                )
            },
            INVALID
        );
        assert_eq!(handle, 456);
    }
}
