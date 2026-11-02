//! Real native client: the Rust C-ABI frontend and the TensorFS-style Rust dependency
//! coexist. No fake allocator, mock CUDA driver or copied allocator implementation.
#[unsafe(no_mangle)]
pub extern "C" fn mixed_rust_identity() -> u64 {
    varena_client::core_identity()
}
#[unsafe(no_mangle)]
pub extern "C" fn mixed_internal_identity() -> u64 {
    varena::abi::vr_v1_core_identity()
}
#[unsafe(no_mangle)]
pub extern "C" fn mixed_rust_error() -> i32 {
    match varena_client::Gpu::open(-1) {
        Err(e) if e.status == varena_client::Status::Invalid => 1,
        _ => 0,
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn mixed_internal_sequence() -> u64 {
    varena::cache_owner::next_generation().expect("normal-process native request sequence")
}
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

struct RetryOwners {
    _gpu: varena_client::Gpu,
    _region: varena_client::Region,
    used: varena_client::Use,
    allocation: varena_client::Allocation,
    pointers: (u64, u64),
}
static RETRY: std::sync::Mutex<Option<RetryOwners>> = std::sync::Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "C" fn mixed_rust_retry_prepare(stream: u64) -> i32 {
    let result = (|| -> varena_client::Result<()> {
        let gpu = varena_client::Gpu::open(0)?;
        // SAFETY: the fixture retains this actual stream through explicit done/free.
        let stream = unsafe { gpu.borrow_stream(stream) };
        let region = gpu.region(&[1, 2, 3, 4])?;
        let used = gpu.use_regions(&[&region], &stream)?;
        let allocation = gpu.alloc(4096, &stream)?;
        let pointers = (used.ptr(0)?, allocation.ptr()?);
        *RETRY.lock().unwrap() = Some(RetryOwners {
            _gpu: gpu,
            _region: region,
            used,
            allocation,
            pointers,
        });
        Ok(())
    })();
    i32::from(result.is_ok())
}
#[unsafe(no_mangle)]
pub extern "C" fn mixed_rust_retry_refused() -> i32 {
    let mut owners = RETRY.lock().unwrap();
    let owners = owners.as_mut().expect("prepared Rust retry owners");
    let capture = |result: varena_client::Result<()>| {
        result.is_err_and(|e| e.status == varena_client::Status::Capturing)
    };
    i32::from(capture(owners.used.done()) && capture(owners.allocation.free()))
}
#[unsafe(no_mangle)]
pub extern "C" fn mixed_rust_retry_finish() -> i32 {
    let result = (|| -> varena_client::Result<()> {
        let mut slot = RETRY.lock().unwrap();
        let owners = slot.as_mut().expect("retry owner was preserved");
        assert_eq!(
            (owners.used.ptr(0)?, owners.allocation.ptr()?),
            owners.pointers
        );
        owners.used.done()?;
        owners.allocation.free()?;
        drop(slot.take());
        Ok(())
    })();
    i32::from(result.is_ok())
}
