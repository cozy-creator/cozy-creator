//! Admission: before a phase runs, its caller lists the ways it can run, most first, each as the
//! Regions it holds and the graph pool it records into. Varena makes room for the first it can,
//! with every lever reclaim has, and holds it: the Regions pinned for the caller's stream, the pool
//! mapped. The caller runs the way admitted and catches nothing; refused, it runs none of them.
use std::sync::Arc;

use crate::working::{self, PoolId};
use crate::{Error, Pin, Region, Result};

/// One way to run: the weights it reads, and the pool it records into with the working peak an
/// eager pass of it measured.
pub struct Want {
    pub regions: Vec<Arc<Region>>,
    pub pool: Option<(PoolId, usize)>,
}

/// The way admitted, held until its Pins go and its pool's use ends.
pub struct Admitted {
    pub index: usize,
    pub pins: Vec<Pin>,
    pub pool_bytes: usize,
}

/// What each way needed beyond what was resident, none of which varena could make room for.
pub struct Refused {
    pub needs: Vec<usize>,
}

fn missing(want: &Want) -> usize {
    let regions: usize = want.regions.iter()
        .filter(|region| !region.is_resident().unwrap_or(false))
        .map(|region| region.allocated_bytes())
        .sum();
    regions + want.pool.map_or(0, |(_, peak)| working::pool_bytes(peak))
}

/// Unused pins go fenced on `stream`, which nothing was submitted to: no context join.
#[allow(unsafe_code)]
fn release(pins: Vec<Pin>, stream: u64) {
    for pin in pins {
        // SAFETY: nothing was submitted through these pins.
        drop(unsafe { pin.into_fenced_region(&[stream]) });
    }
}

fn capacity(error: &Error) -> bool {
    matches!(error, Error::Exhausted { .. } | Error::OutOfMemory { .. })
}

/// The first of `wants` varena can hold for `stream`, or what each needed.
///
/// # Safety
/// `stream` is live in this device's primary context and reads nothing of the Regions before this
/// returns.
#[allow(unsafe_code)]
pub unsafe fn admit(device: i32, stream: u64, wants: &[Want]) -> Result<std::result::Result<Admitted, Refused>> {
    let mut needs = Vec::with_capacity(wants.len());
    for (index, want) in wants.iter().enumerate() {
        needs.push(missing(want));
        let mut pins = Vec::with_capacity(want.regions.len());
        let mut refused = false;
        for region in &want.regions {
            if region.device() != device {
                release(pins, stream);
                return Err(Error::Invalid("an admitted Region belongs to another device"));
            }
            // SAFETY: this function's own contract.
            match unsafe { region.use_on(stream) } {
                Ok(pin) => pins.push(pin),
                Err(error) if capacity(&error) => {
                    refused = true;
                    break;
                }
                Err(error) => {
                    release(pins, stream);
                    return Err(error);
                }
            }
        }
        if refused {
            release(pins, stream);
            continue;
        }
        let Some((pool, peak)) = want.pool else {
            return Ok(Ok(Admitted { index, pins, pool_bytes: 0 }));
        };
        let bytes = working::pool_bytes(peak);
        match working::pool_reserve(device, pool, bytes) {
            Ok(_) => return Ok(Ok(Admitted { index, pins, pool_bytes: bytes })),
            Err(error) => {
                release(pins, stream);
                working::pool_cancel_preparation(device, pool)?;
                if !capacity(&error) {
                    return Err(error);
                }
            }
        }
    }
    Ok(Err(Refused { needs }))
}
