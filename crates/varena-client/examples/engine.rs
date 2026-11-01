//! Standalone Rust ownership client. No Python interpreter or CUDA SDK is linked.
//! Run `--host` for the real ABI/error path without initializing CUDA.
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
    let gpu = Gpu::open(0)?;
    let stream = gpu.default_stream();
    let weights = gpu.region(&[1, 2, 3, 4])?;
    let mut plan = gpu.plan(&[Phase {
        name: "decode",
        regions: &[&weights],
    }])?;
    for _ in 0..2 {
        let pass = plan.phase("decode", "bytes/4/copy/eager")?;
        let used = gpu.use_regions(&[&weights], &stream)?;
        let scratch = gpu.alloc(4096, &stream)?;
        println!("weights={:#x} scratch={:#x}", used.ptr(0)?, scratch.ptr()?);
        scratch.free()?;
        used.done()?;
        pass.complete()?;
        gpu.trim()?;
    }
    println!("{:?}", gpu.stats()?);
    Ok(())
}
