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
pub extern "C" fn mixed_rust_gpu() -> i32 {
    let result = (|| -> varena_client::Result<()> {
        let gpu = varena_client::Gpu::open(0)?;
        let stream = gpu.default_stream();
        let allocated = gpu.alloc(4096, &stream)?;
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
