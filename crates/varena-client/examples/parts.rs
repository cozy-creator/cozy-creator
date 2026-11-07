//! Real GPU client for the public packed-host constructor; no Python or CUDA SDK.
#![allow(unsafe_code)]
#[path = "support/cuda.rs"]
mod cuda;
use varena_client::{Gpu, Status};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let owned_stream = cuda::Stream::new()?;
    {
        let gpu = Gpu::open(0)?;
        let stream = unsafe { gpu.borrow_stream(owned_stream.raw()) };
        // Real safe-client refusals, followed by construction/readback/refill on
        // the same live GPU. No impossible slices or fabricated native payloads.
        let empty_inputs: [&[&[u8]]; 3] = [&[], &[&[]], &[&[], &[]]];
        for parts in empty_inputs {
            match gpu.region_parts(parts) {
                Err(error) => assert_eq!(error.status, Status::Invalid),
                Ok(_) => return Err("empty host-parts aggregate was accepted".into()),
            }
        }
        let mut first = vec![0x17; (2 << 20) + 17];
        let second = [0x39; 257];
        let (region, offsets) = gpu.region_parts(&[&first, &second, &[], &[0x5b]])?;
        assert_eq!(
            offsets,
            [0, (2 << 20) + 256, (2 << 20) + 768, (2 << 20) + 768]
        );
        let mut expected = vec![0; offsets[3] as usize + 1];
        expected[..first.len()].copy_from_slice(&first);
        expected[offsets[1] as usize..offsets[1] as usize + second.len()].copy_from_slice(&second);
        expected[offsets[3] as usize] = 0x5b;
        first.fill(0);
        let mut fixed = None;
        for _ in 0..2 {
            let mut used = gpu.use_regions(&[&region], &stream)?;
            let pointer = used.ptr(0)?;
            assert_eq!(*fixed.get_or_insert(pointer), pointer);
            let mut scratch = gpu.alloc(expected.len() as u64, &stream)?;
            unsafe {
                owned_stream.verify_copy(pointer, scratch.ptr()?, &expected)?;
            }
            scratch.free()?;
            used.done()?;
        }
        println!(
            "{{\"packed_host_refill\":true,\"stable_address\":true,\"empty_aggregate_refusals\":3,\"valid_after_refusals\":true,\"bytes\":{}}}",
            expected.len()
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let stats = varena_client::retirement_stats()?;
        if stats.quarantined != 0 {
            return Err("native retirement quarantined".into());
        }
        if stats.queued == 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err("retirement proof deadline".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    owned_stream.close()?;
    Ok(())
}
