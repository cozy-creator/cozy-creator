//! Standalone Rust ownership client. No Python interpreter or CUDA SDK is linked.
//! Run `--host` for the real ABI/error path without initializing CUDA.
#![allow(unsafe_code)]
#[path = "support/cuda.rs"]
mod cuda;
use varena_client::{Gpu, Phase, Status};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--host") {
        assert_eq!(varena_client::abi_version(), 1);
        match Gpu::open(-1) {
            Err(error) => assert_eq!(error.status, Status::Invalid),
            Ok(_) => panic!("invalid device accepted"),
        }
        println!(
            "{{\"abi\":1,\"core\":{},\"invalid_device\":true}}",
            varena_client::core_identity()
        );
        return Ok(());
    }
    let owned_stream = cuda::Stream::new()?;
    {
        let gpu = Gpu::open(0)?;
        // SAFETY: retained until all checked operations and deferred retirement finish.
        let stream = unsafe { gpu.borrow_stream(owned_stream.raw()) };
        let weights = gpu.region(&[1, 2, 3, 4])?;
        let mut plan = gpu.plan(&[Phase {
            name: "decode",
            regions: &[&weights],
        }])?;
        for _ in 0..2 {
            let mut pass = plan.phase("decode", "bytes/4/copy/eager")?;
            let mut used = gpu.use_regions(&[&weights], &stream)?;
            let mut scratch = gpu.alloc(4096, &stream)?;
            println!("weights={:#x} scratch={:#x}", used.ptr(0)?, scratch.ptr()?);
            // SAFETY: both complete allocations remain retained by these live handles.
            unsafe {
                owned_stream.verify_copy(used.ptr(0)?, scratch.ptr()?, &[1, 2, 3, 4])?;
            }
            scratch.free()?;
            used.done()?;
            pass.complete()?;
            gpu.trim()?;
        }
        println!("{:?}", gpu.stats()?);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let state = varena_client::retirement_stats()?;
        if state.quarantined != 0 {
            return Err("native retirement was quarantined".into());
        }
        if state.queued == 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err("retirement proof deadline; CUDA owners retained".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    owned_stream.close()?;
    Ok(())
}
