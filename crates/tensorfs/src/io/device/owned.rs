//! The raw reader's actual engine/resources after all host submitters close.
use super::*;

pub(in crate::io) struct OwnedRead {
    engine: Arc<Engine>,
    streams: usize,
    owners: Option<Retained>,
    timing: Option<(Option<usize>, Vec<usize>, bool)>,
    closed: bool,
    completed: bool,
    released: bool,
    failed: Option<cuda::Error>,
    stats: ReadStats,
    issued: u64,
    began: Instant,
}

// SAFETY: this exclusive owner retains its Engine/context and every native handle. All
// device operations enter that primary context; no handle is tied to the submitting thread.
unsafe impl Send for OwnedRead {}

impl OwnedRead {
    pub(in crate::io) fn prepare(device: i32, streams: usize) -> cuda::Result<Self> {
        Ok(Self {
            engine: Engine::get(device)?,
            streams: streams.clamp(1, STREAMS),
            owners: None,
            timing: None,
            closed: false,
            completed: false,
            released: false,
            failed: None,
            stats: ReadStats::default(),
            issued: 0,
            began: Instant::now(),
        })
    }

    pub(in crate::io) fn device(&self) -> i32 {
        self.engine.device
    }

    pub(in crate::io) fn streams(&self) -> Vec<u64> {
        self.engine.streams[..self.streams]
            .iter()
            .map(|stream| stream.raw() as u64)
            .collect()
    }

    /// The Source owns every exclusive Fill destination and stages every host input.
    pub(in crate::io) unsafe fn submit(
        &mut self,
        copies: &[Copy<'_>],
        opts: &ReadOpts,
        cancel: &ReadControl,
    ) -> Result<ReadStats, Error> {
        if self.closed || self.owners.is_some() {
            return Err(Error {
                copy: 0,
                source: io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "raw read was already submitted",
                ),
            });
        }
        for (index, copy) in copies.iter().enumerate() {
            if matches!(copy.src, Src::Device { .. })
                || !matches!(copy.dst, Dst::Device {device,..} if device == self.engine.device)
            {
                return Err(Error {
                    copy: index,
                    source: io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "owned raw read requires file/inline/zero into one GPU",
                    ),
                });
            }
        }
        self.began = Instant::now();
        let indexed = copies.iter().copied().enumerate().collect::<Vec<_>>();
        let engine = Arc::clone(&self.engine);
        let result = if indexed.is_empty() {
            Ok(ReadStats::default())
        } else {
            engine.run(&indexed, opts, true, None, Some(cancel), Some(self))
        };
        if !self.closed {
            // Engine refused before constructing RunHold, hence before a writer exists.
            self.close_unsubmitted();
        }
        result
    }

    pub(in crate::io) fn close_unsubmitted(&mut self) {
        if !self.closed {
            self.closed = true;
            self.completed = true;
            self.stats.ns = self.began.elapsed().as_nanos() as u64;
        }
    }

    pub(super) fn close(
        &mut self,
        owners: Retained,
        timing: Option<(Option<usize>, Vec<usize>, bool)>,
        stats: ReadStats,
        issued: u64,
        recording_failure: Option<cuda::Error>,
    ) {
        self.owners = Some(owners);
        self.timing = timing;
        self.stats = stats;
        self.issued = issued;
        self.failed = recording_failure;
        self.closed = true;
    }

    pub(in crate::io) fn closed(&self) -> bool {
        self.closed
    }
    pub(in crate::io) fn issued_bytes(&self) -> u64 {
        self.issued
    }

    pub(in crate::io) fn logical_bytes(&mut self, bytes: u64) {
        self.stats.bytes = bytes;
    }

    fn check(&self) -> cuda::Result<()> {
        if let Some(error) = &self.failed {
            Err(error.clone())
        } else {
            Ok(())
        }
    }

    pub(in crate::io) fn poll(&mut self) -> cuda::Result<Option<ReadStats>> {
        self.check()?;
        let result = self.poll_inner();
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }

    fn poll_inner(&mut self) -> cuda::Result<Option<ReadStats>> {
        if !self.closed {
            return Ok(None);
        }
        if self.completed {
            return Ok(Some(self.stats));
        }
        let (start, ends, recorded) = self.timing.as_ref().ok_or_else(|| {
            cuda::Error::CudaUnavailable("raw read has no closed end-event set".into())
        })?;
        if !recorded || ends.len() != self.streams {
            return Err(cuda::Error::CudaUnavailable(
                "raw read end-event recording is unproven".into(),
            ));
        }
        let owners = self.owners.as_ref().expect("closed read retains events");
        let _ctx = CtxGuard::enter(self.engine.ctx)?;
        for &index in ends {
            let code = unsafe { (cuda::driver()?.event_query)(owners.events[index].raw) };
            if code == 600 {
                return Ok(None);
            }
            cuda::check("cuEventQuery(raw source)", code)?;
        }
        let mut busy_ns = 0;
        if let Some(start) = start {
            for (lane, &end) in ends.iter().enumerate() {
                busy_ns += owners.events[end].since(&owners.events[start + lane])?;
            }
        }
        self.stats.busy_ns = busy_ns;
        self.stats.ns = self.began.elapsed().as_nanos() as u64;
        self.completed = true;
        Ok(Some(self.stats))
    }

    /// Caller owns retirement exclusion and exclusive Resources, outside every mutex.
    pub(in crate::io) fn retire(&mut self) -> cuda::Result<bool> {
        self.check()?;
        let result = self.retire_inner();
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }

    fn retire_inner(&mut self) -> cuda::Result<bool> {
        if self.released {
            return Ok(true);
        }
        if self.poll()?.is_none() {
            return Ok(false);
        }
        if let Some(owners) = &mut self.owners {
            self.engine
                .checked_release(std::slice::from_mut(owners), true)?;
            // Return completed events to the pool without dropping a CUDA owner under its
            // mutex. Overflow destruction is checked outside the pool lock.
            {
                let mut pool = self
                    .engine
                    .events
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                while pool.len() < EVENTS && !owners.events.is_empty() {
                    pool.push(owners.events.pop().expect("nonempty completed event set"));
                }
            }
            while let Some(event) = owners.events.last_mut() {
                event.release_checked()?;
                owners.events.pop();
            }
            for mut slot in owners.slots.drain(..) {
                slot.pending = false;
                slot.recorded = false;
                self.engine.free().push(slot);
            }
            // Raw sources never introduce decoder buffers or foreign producer streams.
            debug_assert!(owners.streams.is_empty() && owners._buffers.is_empty());
        }
        drop(self.owners.take());
        self.released = true;
        Ok(true)
    }
}

impl Drop for OwnedRead {
    fn drop(&mut self) {
        if let Some(owners) = self.owners.take() {
            // Any non-retired path preserves actual resources; no caller-stack GPU drop.
            std::mem::forget(owners);
        }
    }
}
