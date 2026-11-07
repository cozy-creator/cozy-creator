//! Immutable host bytes with one bounded HOST VMM stage and one owned producer stream.
use super::*;

const STAGING: usize = 2 << 20;

#[derive(Clone)]
pub(super) enum Payload {
    Copied(Arc<[u8]>),
    Borrowed(Arc<host_parts::Borrowed>),
}

impl Payload {
    fn len(&self) -> usize {
        match self {
            Self::Copied(bytes) => bytes.len(),
            Self::Borrowed(borrowed) => borrowed.len(),
        }
    }

    /// `to` holds `len` bytes, and `[at, at + len)` lies within this payload.
    unsafe fn read(&self, at: usize, to: *mut u8, len: usize) {
        match self {
            // SAFETY: the bounds above.
            Self::Copied(bytes) => unsafe { std::ptr::copy_nonoverlapping(bytes[at..at + len].as_ptr(), to, len) },
            // SAFETY: the bounds above.
            Self::Borrowed(borrowed) => unsafe { borrowed.read(at, to, len) },
        }
    }
}

/// Host bytes as a Region's Source.
struct Bytes {
    gpu: Arc<Gpu>,
    payload: Payload,
}
impl crate::Source for Bytes {
    fn fill(&self, fill: crate::Fill, consumer: Option<u64>) -> crate::Result<Option<Pin>> {
        let consumer = consumer.ok_or(Error::Busy)?;
        match acquire(&self.gpu, self.payload.clone(), fill, consumer) {
            Ok(pin) => Ok(Some(pin)),
            Err(Failure(status, detail, bytes)) => Err(Error::Source { status, detail, bytes }),
        }
    }
}

pub(super) fn region(gpu: &Arc<Gpu>, payload: Payload) -> crate::Result<Arc<crate::Region>> {
    gpu.core.region(payload.len(), Arc::new(Bytes { gpu: gpu.clone(), payload }))
}

struct State {
    bytes: Payload,
    stream: usize,
    host: crate::cuda::HostAllocation,
    capacity: usize,
    reuse: usize,
}
struct Host {
    gpu: Arc<Gpu>,
    state: Mutex<Option<State>>,
}
struct Claim<'a> {
    host: &'a Host,
    state: Option<State>,
}
impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            *self
                .host
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(state);
        }
    }
}
impl Host {
    fn claim(&self) -> crate::Result<Option<Claim<'_>>> {
        let state = self
            .state
            .lock()
            .map_err(|_| Error::Poisoned("host Source custody mutex".into()))?
            .take();
        Ok(state.map(|state| Claim {
            host: self,
            state: Some(state),
        }))
    }
    fn prepare(&self) -> crate::Result<usize> {
        let mut held = self.claim()?.ok_or(Error::Busy)?;
        let state = held.state.as_mut().expect("exclusive host preparation");
        let driver = self.gpu.core.driver();
        let _context = driver.enter()?;
        state.stream = driver.create_stream()?;
        driver.prepare_host(&mut state.host, state.capacity)?;
        state.reuse = driver.create_event()?;
        Ok(state.stream)
    }
    fn submit(&self, destination: crate::FillDestination) -> crate::Result<()> {
        let mut held = self.claim()?.ok_or(Error::Busy)?;
        let state = held.state.as_mut().expect("exclusive host submission");
        let driver = self.gpu.core.driver();
        let _context = driver.enter()?;
        let mut recorded = false;
        let total = state.bytes.len();
        for offset in (0..total).step_by(state.capacity) {
            let len = state.capacity.min(total - offset);
            if recorded {
                driver.wait_event(state.reuse)?;
            } // Only this stage's prior DMA.
            // SAFETY: the previous DMA completed, and the one HOST VMM stage owns capacity.
            unsafe {
                state.bytes.read(offset, state.host.pointer() as *mut u8, len);
            }
            let pointer = destination
                .ptr
                .checked_add(offset as u64)
                .ok_or(Error::Invalid("host Source address overflow"))?;
            // SAFETY: Source retains the immutable host bytes, HOST VMM stage and exclusive
            // Fill. Its native session retains the fixed stream through actual completion.
            unsafe {
                driver.copy_pinned_async(pointer, state.host.pointer(), len, state.stream)?;
            }
            driver.record(state.reuse, state.stream)?;
            recorded = true;
        }
        // The final recorded copy remains asynchronous. Only native producer events can
        // publish completed contents or permit stage/source retirement.
        Ok(())
    }
    fn retire(&self) -> crate::Result<bool> {
        let Some(mut held) = self.claim()? else {
            return Ok(false);
        };
        let state = held.state.as_mut().expect("exclusive host retirement");
        let driver = self.gpu.core.driver();
        let _context = driver.enter()?;
        driver.release_host(&mut state.host)?;
        if state.reuse != 0 {
            driver.destroy_event(state.reuse)?;
            state.reuse = 0;
        }
        if state.stream != 0 {
            driver.destroy_stream(state.stream)?;
            state.stream = 0;
        }
        drop(held.state.take()); // Host-only immutable bytes; no mutex is held here.
        Ok(true)
    }
}
struct Resource(Arc<Host>);
// SAFETY: preparation submits no work. The fixed producer stream covers every actual
// DMA and staging reuse; native completion precedes retirement. Claim moves actual
// owners out of the mutex before queries, waits or release. Errors retain all leftovers.
unsafe impl crate::FillResource for Resource {
    fn retire(&mut self) -> crate::Result<crate::FillRetirement> {
        self.0.retire().map(|done| {
            if done {
                crate::FillRetirement::Complete
            } else {
                crate::FillRetirement::Pending
            }
        })
    }
}

fn acquire(gpu: &Arc<Gpu>, bytes: Payload, fill: crate::Fill, consumer: u64) -> Result<Pin> {
    let host = Arc::new(Host {
        gpu: gpu.clone(),
        state: Mutex::new(Some(State {
            capacity: bytes.len().min(STAGING),
            bytes,
            stream: 0,
            host: crate::cuda::HostAllocation::default(),
            reuse: 0,
        })),
    });
    let prepared = host.prepare();
    let stream = prepared.as_ref().copied().unwrap_or(0);
    // Even partially prepared owners move into native no-write cleanup. Nothing is
    // destroyed on the caller's stack and there is no host cache or retry byte policy.
    let mut session = unsafe {
        fill.into_async(crate::FillOwners::new(
            if stream == 0 {
                Vec::new()
            } else {
                vec![stream as u64]
            },
            vec![Box::new(Resource(host.clone()))],
        ))
    };
    prepared?;
    let destination = session.begin()?;
    if let Err(error) = host.submit(destination) {
        // This single submitter stopped; record the actual stream tail and retain the
        // last stage through checked abort. A failed seal quarantines the same owners.
        unsafe {
            session.abort(error.clone())?;
        }
        return Err(error.into());
    }
    Ok(unsafe { session.publish_to(consumer)? })
}
