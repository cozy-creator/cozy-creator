//! What a native engine asks before sizing its work. Thin over the core's `room` and split calls.
use super::*;

#[repr(C)]
#[derive(Default)]
pub struct Room {
    pub capacity: u64,
    pub working_live: u64,
    pub pinned: u64,
    pub weights_resident: u64,
    pub free: u64,
}
#[repr(C)]
pub struct SplitChoice {
    pub id: *const c_char,
    pub bytes: u64,
}
#[repr(C)]
#[derive(Default)]
pub struct SplitSelection {
    pub index: u64,
    pub bytes: u64,
    pub measured: u64,
    pub budget: u64,
    pub generation: u64,
}
#[repr(C)]
#[derive(Default)]
pub struct SplitEnd {
    pub peak: u64,
    pub counted: u64,
}

fn sized<T>(out_size: usize) -> Result<()> {
    if out_size != std::mem::size_of::<T>() {
        return Err(invalid("output size mismatch"));
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_room(g: u64, out: *mut Room, out_size: usize) -> i32 {
    boundary(|| {
        sized::<Room>(out_size)?;
        unsafe { output(out)? };
        let room = working::room(gpu(g)?.device())?;
        unsafe {
            out.write(Room {
                capacity: room.capacity as u64,
                working_live: room.working_live as u64,
                pinned: room.pinned as u64,
                weights_resident: room.weights_resident as u64,
                free: room.free as u64,
            });
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_split_select(
    g: u64,
    key: *const c_char,
    choices: *const SplitChoice,
    n: usize,
    stream: u64,
    out: *mut SplitSelection,
    out_size: usize,
) -> i32 {
    boundary(|| {
        sized::<SplitSelection>(out_size)?;
        unsafe { output(out)? };
        let gpu = gpu(g)?;
        let key = unsafe { text(key)? };
        let menu = unsafe { slice(choices, n)? }
            .iter()
            .map(|choice| Ok((unsafe { text(choice.id)? }, vec![size(choice.bytes)?])))
            .collect::<Result<Vec<_>>>()?;
        gpu.eager(stream)?;
        let ids: Vec<String> = menu.iter().map(|(id, _)| id.clone()).collect();
        let chosen = working::split_select(gpu.device(), &key, menu, stream)?;
        unsafe {
            out.write(SplitSelection {
                index: ids.iter().position(|id| *id == chosen.choice_id).expect("copied menu") as u64,
                bytes: chosen.allocation_backing_bytes as u64,
                measured: u64::from(chosen.basis == "measured"),
                budget: chosen.budget_bytes as u64,
                generation: chosen.plan_generation,
            });
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_split_begin(
    id: u64,
    key: *const c_char,
    choice: *const c_char,
    stream: u64,
    generation: u64,
    out: *mut u64,
) -> i32 {
    boundary(|| {
        unsafe { output(out)? };
        let p = plan(id)?;
        let token = lock(&p.phase)?.ok_or_else(|| invalid("a split needs an active phase"))?;
        p.gpu.eager(stream)?;
        let (key, choice) = unsafe { (text(key)?, text(choice)?) };
        let split = working::split_begin(p.gpu.device(), &key, &choice, Some(token), stream, Some(generation))?;
        unsafe { out.write(split) };
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vr_v1_split_end(id: u64, split: u64, submitted: i32, out: *mut SplitEnd, out_size: usize) -> i32 {
    boundary(|| {
        sized::<SplitEnd>(out_size)?;
        unsafe { output(out)? };
        if submitted != 0 && submitted != 1 {
            return Err(invalid("submitted must be 0 or 1"));
        }
        let row = working::split_end(plan(id)?.gpu.device(), split, submitted == 1)?;
        unsafe {
            out.write(SplitEnd {
                peak: row.peak_bytes as u64,
                counted: u64::from(row.pending && row.reason.is_empty()),
            });
        }
        Ok(())
    })
}
