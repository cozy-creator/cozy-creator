//! Immutable host bytes with one bounded pinned stage and one owned producer stream.
use super::*;

const STAGING: usize = 2 << 20;
struct State {
    bytes: Arc<[u8]>,
    stream: usize,
    host: usize,
    capacity: usize,
    reuse: usize,
}
struct Host {
    weight: Arc<Weight>,
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
        let driver = &self.weight.gpu.arena.driver;
        let _context = driver.enter()?;
        state.stream = driver.create_stream()?;
        state.host = driver.allocate_host(state.capacity)?;
        state.reuse = driver.create_event()?;
        Ok(state.stream)
    }
    fn submit(&self, destination: crate::FillDestination) -> crate::Result<()> {
        let mut held = self.claim()?.ok_or(Error::Busy)?;
        let state = held.state.as_mut().expect("exclusive host submission");
        let driver = &self.weight.gpu.arena.driver;
        let _context = driver.enter()?;
        let mut recorded = false;
        for (index, bytes) in state.bytes.chunks(state.capacity).enumerate() {
            if recorded {
                driver.wait_event(state.reuse)?;
            } // Only this stage's prior DMA.
            // SAFETY: the previous DMA completed, and the one pinned stage owns capacity.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), state.host as *mut u8, bytes.len());
            }
            let offset = index
                .checked_mul(state.capacity)
                .ok_or(Error::Invalid("host Source offset overflow"))?;
            let pointer = destination
                .ptr
                .checked_add(offset as u64)
                .ok_or(Error::Invalid("host Source address overflow"))?;
            // SAFETY: Source retains the immutable host bytes, pinned stage and exclusive
            // Fill. Its native session retains the fixed stream through actual completion.
            unsafe {
                driver.copy_pinned_async(pointer, state.host, bytes.len(), state.stream)?;
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
        let driver = &self.weight.gpu.arena.driver;
        let _context = driver.enter()?;
        if state.reuse != 0 {
            driver.destroy_event(state.reuse)?;
            state.reuse = 0;
        }
        if state.stream != 0 {
            driver.destroy_stream(state.stream)?;
            state.stream = 0;
        }
        if state.host != 0 {
            driver.free_host(state.host)?;
            state.host = 0;
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

pub(super) fn acquire(
    weight: &Arc<Weight>,
    bytes: Arc<[u8]>,
    fill: crate::Fill,
    consumer: u64,
) -> Result<Pin> {
    let host = Arc::new(Host {
        weight: weight.clone(),
        state: Mutex::new(Some(State {
            capacity: bytes.len().min(STAGING),
            bytes,
            stream: 0,
            host: 0,
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
    retirement::fill(session.observer());
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
    unsafe {
        session.publish()?;
    }
    match unsafe { weight.region.prepare_on(consumer)? } {
        crate::StreamPreparation::Ordered(pin) | crate::StreamPreparation::Ready(pin) => Ok(pin),
        crate::StreamPreparation::Fill(fill) => {
            std::mem::forget(fill);
            Err(Failure(
                FAULT,
                "published host Source unexpectedly became a Fill".into(),
                0,
            ))
        }
    }
}
