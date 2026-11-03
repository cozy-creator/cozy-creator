//! Explicit pre-body cache service. Allocation recovery and ordinary Use never await it.
use super::{Gpu, Weight};
use crate::{Error, Result, cache_owner, plan, working};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, Default)]
struct Snapshot {
    missing: usize,
    available: usize,
    weights: usize,
    working: usize,
    limit: usize,
    physical_free: usize,
}
impl Snapshot {
    fn deficit(self) -> usize {
        self.missing.saturating_sub(self.available)
    }
}

pub(super) struct Admission {
    // Struct fields drop in declaration order: release exclusion before final owners.
    _retirement: crate::GlobalCaptureGuard,
    gpu: Arc<Gpu>,
    weights: Vec<Arc<Weight>>,
    lease: plan::Lease,
    ids: BTreeSet<u64>,
    generations: BTreeMap<u64, u64>,
    request: Option<cache_owner::ShrinkRequest>,
    request_generation: u64,
    facts: cache_owner::RequestFacts,
    native_released: usize,
    snapshot: Option<Snapshot>,
    current_region: u64,
    refused_bytes: u64,
}

impl Admission {
    pub(super) fn prepare(gpu: Arc<Gpu>, weights: &[Arc<Weight>]) -> Result<Self> {
        if cache_owner::is_dispatcher_thread(gpu.device())? {
            return Err(Error::Invalid(
                "Use preparation cannot await its own cache dispatcher",
            ));
        }
        let retained = weights.to_vec();
        let lease = plan::Lease::acquire(gpu.device())?;
        let ids = weights.iter().map(|weight| weight.region.id()).collect();
        lease.validate_use(&ids)?;
        let generations = weights
            .iter()
            .map(|weight| {
                (
                    weight.region.id(),
                    weight.region.uses.generation.load(Ordering::Acquire),
                )
            })
            .collect();
        let retirement = crate::enter_retirement(gpu.device())?;
        let mut admitted = Self {
            gpu,
            weights: retained,
            lease,
            ids,
            generations,
            _retirement: retirement,
            request: None,
            request_generation: 0,
            facts: cache_owner::RequestFacts::default(),
            native_released: 0,
            snapshot: None,
            current_region: 0,
            refused_bytes: 0,
        };
        let mut result = admitted.service();
        if let Some(request) = &admitted.request {
            if let Err(error) = request.finish_admission() {
                admitted.report("quarantined", "callback_quiescence_unproven");
                // A failed wait cannot drop the exact requested owners or admit capture
                // against an unproven callback. Keep the request and retirement lease too.
                std::mem::forget(admitted);
                return Err(error);
            }
            admitted.facts = request.facts()?;
            if result.is_ok() {
                result = request.admission_result();
            }
        }
        if let Err(error) = result {
            admitted.report("refused", error_kind(&error));
            return Err(error);
        }
        // Quiescence precedes the last readback: a callback's return grants no bytes.
        let final_snapshot = match admitted.read_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                admitted.report("refused", error_kind(&error));
                return Err(error);
            }
        };
        admitted.snapshot = Some(final_snapshot);
        if final_snapshot.deficit() != 0 {
            admitted.report("refused", "capacity");
            return Err(Error::OutOfMemory {
                requested_bytes: final_snapshot.missing,
            });
        }
        Ok(admitted)
    }

    pub(super) fn check_region(&self, weight: &Weight) -> Result<()> {
        self.lease.validate_use(&self.ids)?;
        if self.generations.get(&weight.region.id()).copied()
            != Some(weight.region.uses.generation.load(Ordering::Acquire))
        {
            return Err(Error::Invalid(
                "prepared Use Region fill generation changed",
            ));
        }
        Ok(())
    }

    pub(super) fn begin_region(&mut self, weight: &Weight) -> Result<()> {
        self.current_region = weight.region.id();
        self.check_region(weight)
    }

    // Duplicate handles preserve public indexing but may observe our own earlier fill.
    pub(super) fn acquired(&mut self, weight: &Weight) {
        self.generations.insert(
            weight.region.id(),
            weight.region.uses.generation.load(Ordering::Acquire),
        );
    }

    pub(super) fn finish_acquisition(&self) -> Result<()> {
        for weight in &self.weights {
            self.check_region(weight)?;
        }
        Ok(())
    }

    pub(super) fn completed<T>(&mut self, result: &super::Result<T>) {
        self.refused_bytes = result.as_ref().err().map_or(0, |error| error.2);
        if result.is_err() && self.request_generation == 0 {
            // Diagnostics must not replace the original acquisition error.
            let _ = self.report_regions("after_acquisition_cleanup");
        }
        if self.request.is_some() || self.native_released != 0 || result.is_err() {
            self.report(
                if result.is_ok() {
                    "acquired"
                } else {
                    "acquisition_refused"
                },
                match result {
                    Ok(_) => "ok",
                    Err(error) => match error.0 {
                        super::EXHAUSTED => "capacity",
                        super::BUSY => "busy",
                        super::CAPTURING => "capture",
                        super::SOURCE_ERROR => "source",
                        super::FAULT => "fault",
                        _ => "invalid",
                    },
                },
            );
        }
    }

    fn read_snapshot(&self) -> Result<Snapshot> {
        let captures = crate::capture_counts()?;
        crate::ensure_recovery_allowed(&captures, self.gpu.device())?;
        self.lease.validate_use(&self.ids)?;
        let mut missing = 0usize;
        let mut seen = BTreeSet::new();
        for weight in &self.weights {
            if !seen.insert(weight.region.id()) {
                continue;
            }
            self.check_region(weight)?;
            let state = weight.region.arena.lock()?;
            let entry = state
                .entries
                .get(&weight.region.id())
                .ok_or(Error::Invalid("prepared Use Region left its Arena"))?;
            if entry.uses.state() == crate::Contents::Retired {
                return Err(Error::Invalid("prepared Use Region is retired"));
            }
            // Mapped pending producer backing is already charged. Actual acquisition
            // retains prepare_on's Published ordering / Loading refusal semantics.
            if entry.handle.is_none() {
                missing = missing
                    .checked_add(entry.bytes)
                    .ok_or(Error::Invalid("prepared Use demand overflow"))?;
            }
        }
        let room = &self.gpu.arena.room;
        let weights = room.pages.load(Ordering::Acquire);
        let working = room.working.load(Ordering::Acquire);
        let limit = room
            .limit
            .load(Ordering::Acquire)
            .min(room.ceiling.load(Ordering::Acquire));
        let _context = self.gpu.arena.driver.enter()?;
        let (physical_free, _) = self.gpu.arena.driver.memory_info()?;
        let available = physical_free.min(limit.saturating_sub(weights.saturating_add(working)));
        Ok(Snapshot {
            missing,
            available,
            weights,
            working,
            limit,
            physical_free,
        })
    }

    fn collect(&self, wanted: usize) -> Result<usize> {
        if wanted == 0 {
            return Ok(0);
        }
        self.lease.validate_use(&self.ids)?;
        let mut completed = working::reclaim_completed(self.gpu.device(), wanted)?;
        if completed < wanted {
            let captures = crate::capture_counts()?;
            crate::ensure_recovery_allowed(&captures, self.gpu.device())?;
            self.lease.validate_use(&self.ids)?;
            for arena in self.lease.arenas() {
                if completed >= wanted {
                    break;
                }
                arena.reclaim_ready_excluding_counted(
                    &captures,
                    wanted - completed,
                    usize::MAX,
                    crate::ReclaimReason::Allocator,
                    &mut completed,
                    &self.ids,
                )?;
            }
        }
        Ok(completed)
    }

    fn service(&mut self) -> Result<()> {
        let mut current = self.read_snapshot()?;
        self.snapshot = Some(current);
        if current.deficit() == 0 {
            return Ok(());
        }
        self.request_generation = cache_owner::next_generation()?;
        self.report("pressure", "observed");
        self.report_regions("before_service")?;
        self.native_released = self.collect(current.deficit())?;
        current = self.read_snapshot()?;
        self.snapshot = Some(current);
        if current.deficit() == 0 {
            return Ok(());
        }
        let wanted = current.deficit();
        self.request = cache_owner::request_shrink(
            self.gpu.device(),
            cache_owner::RequestKey {
                cause: cache_owner::Cause::Admission,
                generation: self.request_generation,
            },
            wanted,
        )?;
        if let Some(request) = &self.request {
            self.facts = request.facts()?;
        }
        let Some(request) = self.request.clone() else {
            self.report("request_unavailable", "capacity");
            return Err(Error::OutOfMemory {
                requested_bytes: current.missing,
            });
        };
        self.report("requested", "pending");
        let (mut epoch, mut serviced) = (0, 0usize);
        loop {
            self.facts = request.wait_admission_action(epoch)?;
            let released = self.collect(current.deficit())?;
            self.native_released = self.native_released.saturating_add(released);
            serviced = serviced.saturating_add(released).min(wanted);
            current = self.read_snapshot()?;
            self.snapshot = Some(current);
            self.report("serviced", "observed");
            if current.deficit() == 0 {
                return Ok(());
            }
            if self.facts.finished || self.facts.cancelled {
                return Err(Error::OutOfMemory {
                    requested_bytes: current.missing,
                });
            }
            epoch = self.facts.action_epoch;
            request.service_progress(serviced, epoch)?;
        }
    }

    // One pressure-only row per exact requested identity; never a global census.
    // Gather scalar metadata under native guards, then release them before writing.
    fn report_regions(&self, sample: &str) -> Result<()> {
        let rows = {
            let _captures = crate::capture_counts()?;
            self.lease.validate_use(&self.ids)?;
            let mut rows = Vec::with_capacity(self.ids.len());
            let mut seen = BTreeSet::new();
            for weight in &self.weights {
                let region = &weight.region;
                if !seen.insert(region.id()) {
                    continue;
                }
                let state = region.arena.lock()?;
                let entry = state
                    .entries
                    .get(&region.id())
                    .ok_or(Error::Invalid("prepared Use census Region left its Arena"))?;
                let contents = match entry.uses.state() {
                    crate::Contents::Cold => "cold",
                    crate::Contents::Loading => "loading",
                    crate::Contents::Published => "published",
                    crate::Contents::Ready => "ready",
                    crate::Contents::Retired => "retired",
                };
                rows.push((
                    region.arena_id(),
                    region.id(),
                    entry.requested_bytes,
                    entry.bytes,
                    if entry.handle.is_some() {
                        entry.bytes
                    } else {
                        0
                    },
                    entry.uses.pins(),
                    contents,
                    entry.uses.generation.load(Ordering::Acquire),
                    entry.uses.unjoined.load(Ordering::Acquire),
                ));
            }
            rows
        };
        for (arena, region, logical, rounded, mapped, pins, contents, generation, unjoined) in rows
        {
            let _ = writeln!(
                std::io::stderr().lock(),
                "VARENA_USE_ADMISSION_V1 {{\"event\":\"selected_region\",\"sample\":\"{sample}\",\"device\":{},\"request_generation\":{},\"arena\":{arena},\"region\":{region},\"logical\":{logical},\"rounded\":{rounded},\"mapped\":{mapped},\"pins\":{pins},\"contents\":\"{contents}\",\"fill_generation\":{generation},\"unjoined\":{unjoined}}}",
                self.gpu.device(),
                self.request_generation
            );
        }
        Ok(())
    }

    // Pressure + selected identities + request + one row per owner action + terminal.
    // Global releases are explicitly an observation, never this operation's service credit.
    fn report(&self, event: &str, outcome: &str) {
        let s = self.snapshot.unwrap_or_default();
        let (generation, inventory, active) = self.lease.identity();
        let (plan, phase) = active.unwrap_or_default();
        let _ = writeln!(
            std::io::stderr().lock(),
            "VARENA_USE_ADMISSION_V1 {{\"event\":\"{event}\",\"outcome\":\"{outcome}\",\"device\":{},\"regions\":{},\"plan\":{plan},\"phase\":{phase},\"plan_generation\":{generation},\"inventory_generation\":{inventory},\"snapshot_available\":{},\"needed\":{},\"headroom\":{},\"weights\":{},\"working\":{},\"limit\":{},\"physical_free\":{},\"request_generation\":{},\"requested\":{},\"queued\":{},\"running\":{},\"finished\":{},\"cancelled\":{},\"actions\":{},\"declined\":{},\"failed\":{},\"native_service_released\":{},\"request_serviced\":{},\"observed_global_releases\":{},\"current_region\":{},\"refused_bytes\":{},\"last_action_owner\":{}}}",
            self.gpu.device(),
            self.ids.len(),
            self.snapshot.is_some(),
            s.missing,
            s.available,
            s.weights,
            s.working,
            s.limit,
            s.physical_free,
            self.request_generation,
            self.facts.requested_bytes,
            self.facts.queued,
            self.facts.running,
            self.facts.finished,
            self.facts.cancelled,
            self.facts.action_epoch,
            self.facts.declined,
            self.facts.failed,
            self.native_released,
            self.facts.serviced_bytes,
            self.facts.observed_release_bytes,
            self.current_region,
            self.refused_bytes,
            self.facts.last_action_owner
        );
    }
}

fn error_kind(error: &Error) -> &'static str {
    match error {
        Error::OutOfMemory { .. } | Error::Exhausted { .. } => "capacity",
        Error::Busy => "busy",
        Error::CaptureActive => "capture",
        Error::Callback(_) => "callback",
        Error::Poisoned(_) | Error::DeviceFault { .. } => "fault",
        _ => "invalid",
    }
}
