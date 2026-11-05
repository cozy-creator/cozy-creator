//! Real native client over the public C ABI: it builds against a wheel's core on a pod.
#![deny(unsafe_code)]

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn mixed_rust_identity() -> u64 {
    varena_client::core_identity()
}
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn mixed_rust_error() -> i32 {
    match varena_client::Gpu::open(-1) {
        Err(e) if e.status == varena_client::Status::Invalid => 1,
        _ => 0,
    }
}
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn mixed_rust_gpu(stream: u64) -> i32 {
    let result = (|| -> varena_client::Result<()> {
        let gpu = varena_client::Gpu::open(0)?;
        // SAFETY: the fixture retains this actual stream through explicit done/free.
        let stream = unsafe { gpu.borrow_stream(stream) };
        let mut allocated = gpu.alloc(4096, &stream)?;
        if gpu.stats()?.working_live < 4096 {
            return Err(varena_client::Error {
                status: varena_client::Status::Invalid,
                message: "native allocation missing from shared stats".into(),
                requested_bytes: 0,
            });
        }
        allocated.free()?;
        Ok(())
    })();
    i32::from(result.is_ok())
}
/// # Safety
/// `out` is a live `vr_room_v1`.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mixed_rust_room(out: *mut varena_client::Room) -> i32 {
    let room = varena_client::Gpu::open(0).and_then(|gpu| gpu.room());
    let Ok(room) = room else { return 0 };
    // SAFETY: the caller's contract above.
    unsafe { out.write(room) };
    1
}
