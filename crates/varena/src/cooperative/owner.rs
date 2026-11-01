//! Native donor service. Only this thread owns Peer and its retained GPU resources.
//! Allocator lookup references contain host metadata only, never a GPU destructor or join.
use super::pressure::{Boundary, Pressure};
use super::{Peer, Result, Service, deferred};
use crate::ledger;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak, mpsc};
use std::time::{Duration, Instant};

const CADENCE: Duration = Duration::from_millis(500);
const COORDINATION_BUDGET: Duration = crate::RECOVERY_BUDGET;

#[derive(Clone, Copy, Debug)]
pub enum RecoveryOutcome {
    Released,
    Changed,
    Exhausted,
    BudgetExhausted,
    LocalReleaseObserved,
}

impl RecoveryOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Released => "released",
            Self::Changed => "round_changed",
            Self::Exhausted => "peer_reclaim_exhausted",
            Self::BudgetExhausted => "peer_wait_budget_exhausted",
            Self::LocalReleaseObserved => "local_physical_release_observed",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RecoveryReport {
    pub outcome: RecoveryOutcome,
    pub requested_bytes: usize,
    /// Newly observed round progress, never capacity credit; local waiters may share it.
    pub released_bytes: u64,
    pub round: Option<u64>,
    pub completed_bytes: u64,
    pub elapsed_ns: u64,
}

#[derive(Default)]
struct AttemptState {
    observed: Option<(u64, u64)>,
    result: Option<RecoveryReport>,
}

struct Attempt {
    requested: usize,
    deadline: Instant,
    observed: Option<(u64, u64)>,
    state: Mutex<AttemptState>,
}

impl Attempt {
    fn lock(&self) -> crate::Result<MutexGuard<'_, AttemptState>> {
        self.state
            .lock()
            .map_err(|_| crate::Error::Poisoned("peer recovery state mutex".into()))
    }

    fn report(
        &self,
        outcome: RecoveryOutcome,
        released_bytes: u64,
        observed: Option<(u64, u64)>,
    ) -> RecoveryReport {
        report(
            self.requested,
            self.deadline,
            outcome,
            released_bytes,
            observed,
        )
    }
}

fn report(
    requested_bytes: usize,
    deadline: Instant,
    outcome: RecoveryOutcome,
    released_bytes: u64,
    observed: Option<(u64, u64)>,
) -> RecoveryReport {
    RecoveryReport {
        outcome,
        requested_bytes,
        released_bytes,
        round: observed.map(|value| value.0),
        completed_bytes: observed.map_or(0, |value| value.1),
        elapsed_ns: (Instant::now() - (deadline - COORDINATION_BUDGET))
            .as_nanos()
            .min(u64::MAX as u128) as u64,
    }
}

#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub active: bool,
    pub stopping: bool,
    pub retired: bool,
    pub published: u64,
    pub serviced: u64,
    pub deferred: u64,
    pub physical_bytes: u64,
    pub last_donation: Option<Service>,
    pub failed: Option<String>,
    pub recoveries: u64,
    pub last_recovery: Option<RecoveryReport>,
    pub active_recoveries: usize,
    pub pending_withdrawals: usize,
    pub withdrawn_rounds: u64,
    pub last_withdrawn_round: Option<u64>,
    pub demand_clear_pending: bool,
    pub pressure: super::PressureFacts,
    /// Local execution knowledge; the shared row carries the most recent publication.
    pub activity: ledger::Activity,
}

enum Command {
    Configure(ledger::Sharing, i32, mpsc::SyncSender<Result<()>>),
    Snapshot(mpsc::SyncSender<Result<ledger::Snapshot>>),
    CudaCalls(mpsc::SyncSender<Result<u64>>),
    Boundary(Boundary, mpsc::SyncSender<Result<()>>),
}

#[derive(Default)]
struct State {
    stopping: bool,
    retired: bool,
    generation: u64,
    last_use: u64,
    commands: VecDeque<Command>,
    facts: Facts,
    attempt: Option<Arc<Attempt>>,
    next_demand: u64,
    demands: BTreeMap<u64, Instant>,
    owned_round: Option<u64>,
    withdrawals: BTreeSet<u64>,
    want_published: bool,
    clear_want: bool,
    // Pressure yields ownership before any physical-allocation recovery starts. It never
    // withdraws a round used by an allocator, or borrows that allocator's latency budget.
    pressure_round: Option<u64>,
}

fn queue_withdrawal(state: &mut State, round: u64) {
    state.withdrawals.insert(round);
    if state.owned_round == Some(round) {
        state.owned_round = None;
    }
    state.generation = state.generation.wrapping_add(1);
}

fn retire_unused_round(state: &mut State) {
    if !state
        .demands
        .values()
        .any(|deadline| *deadline > Instant::now())
    {
        if state.want_published {
            state.clear_want = true;
            state.want_published = false;
            state.generation = state.generation.wrapping_add(1);
        }
        if let Some(round) = state.owned_round {
            queue_withdrawal(state, round);
        }
    }
}

// No Peer, Arena, Driver, Python reference or JoinHandle may be stored here.
struct Control {
    pid: u32,
    state: Mutex<State>,
    changed: Condvar,
}

/// A local recovery lifetime. Drop only queues host metadata; the worker owns ledger I/O.
pub(crate) struct Participation {
    control: Arc<Control>,
    id: u64,
}

impl Drop for Participation {
    fn drop(&mut self) {
        if self.control.pid != std::process::id() {
            return;
        }
        let mut state = self
            .control
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.demands.remove(&self.id);
        retire_unused_round(&mut state);
        self.control.changed.notify_all();
    }
}

impl Control {
    fn new() -> Self {
        Self {
            pid: std::process::id(),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        if self.pid != std::process::id() {
            return Err(ledger::Error::Inherited.into());
        }
        self.state
            .lock()
            .map_err(|_| crate::Error::Poisoned("native owner control mutex".into()).into())
    }

    fn stop(&self) {
        if self.pid != std::process::id() {
            return;
        }
        // Cancellation must disconnect waiters even if a panicked worker poisoned metadata.
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.stopping = true;
        state.facts.active = false;
        state.commands.clear();
        self.changed.notify_all();
    }

    fn call<T>(&self, command: impl FnOnce(mpsc::SyncSender<Result<T>>) -> Command) -> Result<T> {
        let (send, receive) = mpsc::sync_channel(1);
        {
            let mut state = self.lock()?;
            check(&state)?;
            state.commands.push_back(command(send));
            self.changed.notify_all();
        }
        receive.recv().map_err(|_| {
            crate::Error::Poisoned("native owner stopped during metadata call".into())
        })?
    }
}

fn check(state: &State) -> Result<()> {
    if let Some(reason) = &state.facts.failed {
        return Err(crate::Error::Poisoned(format!("native cooperation failed: {reason}")).into());
    }
    if state.stopping || !state.facts.active {
        return Err(crate::Error::Invalid("native cooperation owner is closed").into());
    }
    Ok(())
}

static OWNERS: OnceLock<Mutex<BTreeMap<i32, Weak<Control>>>> = OnceLock::new();

fn registry() -> Result<MutexGuard<'static, BTreeMap<i32, Weak<Control>>>> {
    crate::check_process()?; // Before touching a possibly inherited mutex.
    OWNERS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| crate::Error::Poisoned("native owner registry mutex".into()).into())
}

fn native(error: super::Error) -> crate::Error {
    match error {
        super::Error::Native(error) => error,
        super::Error::Ledger(error) => {
            crate::Error::Poisoned(format!("native cooperation: {error}"))
        }
    }
}

/// Called after cache callbacks/reclaim, outside all allocator/arena/ledger guards.
/// Only host Control is acquired; callback return itself supplies no capacity credit.
pub(crate) fn wake_cache_actions(device: i32) -> crate::Result<()> {
    let control = registry().map_err(native)?.get(&device).and_then(Weak::upgrade);
    if let Some(control) = control {
        let mut state = control.lock().map_err(native)?;
        state.generation = state.generation.wrapping_add(1);
        control.changed.notify_all();
    }
    Ok(())
}

/// The only allocator entry into cooperation. All references acquired here own host data.
pub(crate) fn recover(
    device: i32,
    bytes: usize,
    deadline: &mut Option<Instant>,
    observed: &mut Option<(u64, u64)>,
    participation: &mut Option<Participation>,
    local_release: Option<u64>,
) -> crate::Result<crate::PeerProgress> {
    {
        let captures = crate::capture_counts()?;
        crate::ensure_recovery_allowed(&captures, device)?;
    }
    let control = registry()
        .map_err(native)?
        .get(&device)
        .and_then(Weak::upgrade);
    let Some(control) = control else {
        return Ok(crate::PeerProgress::Unavailable);
    };
    let room = crate::room(device)?; // host metadata, acquired before the control mutex
    let limit = deadline.get_or_insert_with(|| Instant::now() + COORDINATION_BUDGET);
    'attempt: loop {
        let mut state = control.lock().map_err(native)?;
        check(&state).map_err(native)?;
        if let Some(round) = state.pressure_round.take() {
            queue_withdrawal(&mut state, round);
        }
        // Expired/finished demand cannot be revived by a new allocation arriving before
        // the worker has committed the queued withdrawal.
        retire_unused_round(&mut state);
        let had_demand = state
            .demands
            .values()
            .any(|deadline| *deadline > Instant::now());
        if let Some(demand) = participation.as_ref() {
            if !Arc::ptr_eq(&demand.control, &control) {
                return Err(crate::Error::Invalid("recovery changed native owner"));
            }
        } else {
            let id = state.next_demand;
            state.next_demand = id
                .checked_add(1)
                .ok_or(crate::Error::Invalid("recovery identity overflow"))?;
            state.demands.insert(id, *limit);
            *participation = Some(Participation {
                control: Arc::clone(&control),
                id,
            });
        }
        control.changed.notify_all();
        if Instant::now() >= *limit {
            state.facts.last_recovery = Some(report(
                bytes,
                *limit,
                RecoveryOutcome::BudgetExhausted,
                0,
                *observed,
            ));
            return Ok(crate::PeerProgress::FinalRetry);
        }
        let pending = match state.attempt.as_ref() {
            Some(attempt)
                if had_demand
                    && attempt.deadline > Instant::now()
                    && attempt.lock()?.result.is_none() =>
            {
                Some(Arc::clone(attempt))
            }
            _ => None,
        };
        let attempt = if let Some(attempt) = pending {
            *limit = (*limit).min(attempt.deadline); // Joining cannot extend an episode.
            attempt
        } else {
            let attempt = Arc::new(Attempt {
                requested: bytes,
                deadline: *limit,
                observed: *observed,
                state: Mutex::default(),
            });
            state.attempt = Some(Arc::clone(&attempt));
            state.facts.recoveries += 1;
            state.generation = state.generation.wrapping_add(1);
            control.changed.notify_all();
            attempt
        };
        state.demands.insert(
            participation.as_ref().expect("registered recovery").id,
            *limit,
        );
        loop {
            check(&state).map_err(native)?;
            if local_release.is_some_and(|before| room.released.load(std::sync::atomic::Ordering::Acquire) > before) {
                state.facts.last_recovery = Some(report(bytes, *limit,
                    RecoveryOutcome::LocalReleaseObserved, 0, None));
                return Ok(crate::PeerProgress::Retry);
            }
            if let Some(mut report) = attempt.lock()?.result {
                if let Some(round) = report.round {
                    let prior = observed
                        .filter(|(id, _)| *id == round)
                        .map_or(0, |(_, done)| done);
                    if matches!(report.outcome, RecoveryOutcome::Released) {
                        if report.completed_bytes <= prior {
                            // Another local caller consumed this same observation before its
                            // latest driver failure. Wait for new progress, never spend it twice.
                            drop(state);
                            continue 'attempt;
                        }
                        report.released_bytes = report.completed_bytes - prior;
                    }
                    *observed = Some((round, report.completed_bytes));
                }
                state.facts.last_recovery = Some(report);
                return Ok(match report.outcome {
                    RecoveryOutcome::Released | RecoveryOutcome::Changed | RecoveryOutcome::LocalReleaseObserved => {
                        crate::PeerProgress::Retry
                    }
                    RecoveryOutcome::Exhausted | RecoveryOutcome::BudgetExhausted => {
                        crate::PeerProgress::FinalRetry
                    }
                });
            }
            let now = Instant::now();
            if now >= *limit {
                let known = attempt.lock()?.observed.or(*observed);
                state.facts.last_recovery = Some(report(
                    bytes,
                    *limit,
                    RecoveryOutcome::BudgetExhausted,
                    0,
                    known,
                ));
                return Ok(crate::PeerProgress::FinalRetry);
            }
            (state, _) = control
                .changed
                .wait_timeout(state, *limit - now)
                .map_err(|_| crate::Error::Poisoned("peer recovery wait mutex".into()))?;
        }
    }
}

/// Lifecycle handle, never retained by Room or Arena. Drop only requests asynchronous stop.
/// Explicit close may wait for final GPU retirement, outside application/allocator locks.
pub struct Owner {
    device: i32,
    id: ledger::RowId,
    control: Arc<Control>,
}

impl Owner {
    pub fn open(
        device: i32,
        descriptor: ledger::Device,
        directory: &Path,
        sharing: ledger::Sharing,
        priority: i32,
    ) -> Result<Self> {
        let control = Arc::new(Control::new());
        {
            let mut owners = registry()?;
            if owners.get(&device).and_then(Weak::upgrade).is_some() {
                return Err(crate::Error::Busy.into());
            }
            owners.insert(device, Arc::downgrade(&control));
        }
        // Even failed thread creation cannot drop GPU ownership on the caller's stack.
        let worker = Arc::clone(&control);
        let directory = directory.to_path_buf();
        let (ready, started) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name(format!("varena-owner-{device}"))
            .spawn(
                move || match Peer::open(device, descriptor, &directory, sharing, priority) {
                    Ok(peer) => {
                        if let Ok(mut state) = worker.lock() {
                            state.facts.active = true;
                        }
                        if ready.send(Ok(peer.id())).is_err() {
                            worker.stop(); // The opening caller no longer owns this service.
                        }
                        run(peer, worker, descriptor);
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                    }
                },
            )
            .map_err(ledger::Error::Io)?;
        let id = started
            .recv()
            .map_err(|_| crate::Error::Poisoned("native owner startup failed".into()))??;
        Ok(Self {
            device,
            id,
            control,
        })
    }

    pub fn id(&self) -> ledger::RowId {
        self.id
    }

    pub fn wake(&self, last_use: u64) -> Result<()> {
        let mut state = self.control.lock()?;
        check(&state)?;
        state.last_use = state.last_use.max(last_use);
        state.generation = state.generation.wrapping_add(1);
        self.control.changed.notify_all();
        Ok(())
    }

    pub fn facts(&self) -> Result<Facts> {
        let state = self.control.lock()?;
        let mut facts = state.facts.clone();
        facts.stopping = state.stopping;
        facts.retired = state.retired;
        facts.active_recoveries = state
            .demands
            .values()
            .filter(|deadline| **deadline > Instant::now())
            .count();
        facts.pending_withdrawals = state.withdrawals.len();
        facts.demand_clear_pending = state.clear_want;
        drop(state);
        if let Some(suppressed) = crate::working::peer::prefetch_suppressed(self.device)? {
            facts.pressure.suppressed = suppressed;
        }
        Ok(facts)
    }

    /// Real request lifecycle, carrying no byte target. Calls detach/complete before
    /// application work; overlapping or VMM-changing windows cannot train opaque reserve.
    pub fn boundary(&self, boundary: Boundary) -> Result<()> {
        self.control
            .call(|reply| Command::Boundary(boundary, reply))
    }

    pub fn configure(&self, sharing: ledger::Sharing, priority: i32) -> Result<()> {
        self.control
            .call(|reply| Command::Configure(sharing, priority, reply))
    }

    pub fn snapshot(&self) -> Result<ledger::Snapshot> {
        self.control.call(Command::Snapshot)
    }

    pub fn cuda_calls(&self) -> Result<u64> {
        self.control.call(Command::CudaCalls)
    }

    pub fn close(&self) -> Result<()> {
        drop(self.control.lock()?); // Process check precedes stop and capture registry access.
        self.control.stop();
        if crate::cache_owner::is_dispatcher_thread(self.device)? {
            return Err(crate::Error::Busy.into());
        }
        loop {
            {
                let captures = crate::capture_counts()?;
                crate::ensure_recovery_allowed(&captures, self.device)?;
            }
            let state = self.control.lock()?;
            if let Some(error) = &state.facts.failed {
                return Err(crate::Error::Poisoned(error.clone()).into());
            }
            if state.retired {
                return Ok(());
            }
            // The worker owns retirement; no Peer/JoinHandle can be destroyed on this stack.
            drop(
                self.control
                    .changed
                    .wait_timeout(state, CADENCE)
                    .map_err(|_| crate::Error::Poisoned("native owner close mutex".into()))?,
            );
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        // A private-VM child must not touch its parent's inherited synchronization state.
        if self.control.pid == std::process::id() {
            self.control.stop();
        }
    }
}

fn command(peer: &Peer, control: &Control, pressure: &mut Pressure, command: Command) {
    match command {
        Command::Configure(sharing, priority, reply) => {
            let result = peer.configure(sharing, priority).and_then(|()| {
                if sharing == ledger::Sharing::Dedicated {
                    crate::working::peer::pressure(peer.device, false, true)?;
                    let mut state = control.lock()?;
                    if let Some(round) = state.pressure_round.take() {
                        queue_withdrawal(&mut state, round);
                        control.changed.notify_all();
                    }
                }
                Ok(())
            });
            let _ = reply.send(result);
        }
        Command::Snapshot(reply) => {
            let _ = reply.send(peer.snapshot());
        }
        Command::CudaCalls(reply) => {
            let _ = reply.send(peer.cuda_calls());
        }
        Command::Boundary(kind, reply) => {
            let result = (|| {
                // A capture/deferred sample cannot block lifecycle cleanup or supply an H
                // observation. Unknown accounting simply makes this window unqualified.
                match pressure.observe(peer) {
                    Err(error) if deferred(&error) => pressure.invalidate_observation(),
                    result => {
                        result?;
                    }
                }
                pressure.boundary(kind)?;
                let activity = pressure.activity();
                peer.set_activity(activity)?;
                let mut state = control.lock()?;
                state.facts.activity = activity;
                state.facts.pressure = pressure.facts.clone();
                state.generation = state.generation.wrapping_add(1);
                control.changed.notify_all();
                Ok(())
            })();
            let _ = reply.send(result);
        }
    }
}

fn tick(peer: &Peer, control: &Control, used: u64) -> Result<()> {
    let reading = peer.publish(used)?;
    {
        let mut state = control.lock()?;
        state.facts.published += 1;
        state.facts.physical_bytes = reading.physical.total()?;
        state.facts.deferred += u64::from(!reading.refreshed);
    }
    let service = peer.service()?;
    let mut state = control.lock()?;
    state.facts.serviced += 1;
    state.facts.deferred += u64::from(service.deferred);
    if let Some(reading) = service.accounting {
        state.facts.physical_bytes = reading.physical.total()?;
    }
    if service.round.is_some() {
        state.facts.last_donation = Some(service);
    }
    Ok(())
}

fn finish_attempt(control: &Control, attempt: &Attempt, report: RecoveryReport) -> Result<()> {
    {
        let mut state = attempt.lock()?;
        if state.result.is_some() {
            return Ok(());
        }
        state.result = Some(report);
    }
    // Never nest an Attempt guard with Control: callers inspect attempts under Control.
    let mut state = control.lock()?;
    state.facts.last_recovery = Some(report);
    control.changed.notify_all();
    Ok(())
}

fn withdraw_tick(peer: &Peer, control: &Control) -> Result<()> {
    let (pending, clear_want) = {
        let mut state = control.lock()?;
        retire_unused_round(&mut state);
        (
            state.withdrawals.iter().copied().collect::<Vec<_>>(),
            state.clear_want,
        )
    };
    if clear_want {
        peer.clear_want()?;
        let mut state = control.lock()?;
        state.clear_want = false;
        control.changed.notify_all();
    }
    for generation in pending {
        let changed = peer.withdraw(generation)?;
        let mut state = control.lock()?;
        state.withdrawals.remove(&generation);
        if changed {
            state.facts.withdrawn_rounds += 1;
            state.facts.last_withdrawn_round = Some(generation);
        }
        control.changed.notify_all();
    }
    Ok(())
}

fn own_round(
    peer: &Peer,
    control: &Control,
    attempt: &Arc<Attempt>,
    round: &ledger::Round,
    asked: bool,
) -> Result<bool> {
    let mut state = control.lock()?;
    if asked {
        state.want_published = true;
    }
    let current = state
        .attempt
        .as_ref()
        .is_some_and(|value| Arc::ptr_eq(value, attempt));
    let wanted = current
        && attempt.deadline > Instant::now()
        && state
            .demands
            .values()
            .any(|deadline| *deadline > Instant::now());
    if !wanted && state.want_published {
        state.clear_want = true;
        state.want_published = false;
        state.generation = state.generation.wrapping_add(1);
        control.changed.notify_all();
    }
    if round.requester == peer.id() {
        if wanted && !state.withdrawals.contains(&round.generation) {
            state.owned_round = Some(round.generation);
        } else {
            // An ask may finish just after its last waiter expires or a new attempt replaces
            // it. Record that exact generation for retirement, never give it to a new caller.
            queue_withdrawal(&mut state, round.generation);
            control.changed.notify_all();
        }
    }
    Ok(state.withdrawals.is_empty() && !state.clear_want)
}

fn recover_tick(peer: &Peer, control: &Control) -> Result<()> {
    let attempt = {
        let mut state = control.lock()?;
        retire_unused_round(&mut state);
        if !state.withdrawals.is_empty() || state.clear_want {
            control.changed.notify_all();
            return Ok(());
        }
        state.attempt.clone()
    };
    let Some(attempt) = attempt else {
        return Ok(());
    };
    let observed = {
        let state = attempt.lock()?;
        if state.result.is_some() {
            return Ok(());
        }
        state.observed
    };
    let expired = || {
        attempt.report(
            RecoveryOutcome::BudgetExhausted,
            0,
            observed.or(attempt.observed),
        )
    };
    if Instant::now() >= attempt.deadline {
        return finish_attempt(control, &attempt, expired());
    }
    let round = if let Some((generation, completed)) = observed {
        let table = peer.snapshot()?;
        match table.round {
            Some(round) if round.generation == generation => round,
            _ => {
                return finish_attempt(
                    control,
                    &attempt,
                    attempt.report(RecoveryOutcome::Changed, 0, Some((generation, completed))),
                );
            }
        }
    } else {
        let sample = peer.sample(ledger::monotonic_ns()?)?;
        if Instant::now() >= attempt.deadline {
            return finish_attempt(control, &attempt, expired());
        }
        peer.ask(sample.generation, attempt.requested as u64)?
    };
    if !own_round(peer, control, &attempt, &round, observed.is_none())? {
        return Ok(());
    }
    if Instant::now() >= attempt.deadline {
        return finish_attempt(control, &attempt, expired());
    }
    let completed = round.completed()?;
    let previous = observed
        .or(attempt.observed)
        .filter(|(id, _)| *id == round.generation)
        .map_or(0, |(_, bytes)| bytes);
    if completed < previous {
        return Err(crate::Error::Poisoned("peer round completion regressed".into()).into());
    }
    let next = Some((round.generation, completed));
    let outcome = if completed > previous {
        Some(RecoveryOutcome::Released)
    } else if round.aborted {
        Some(RecoveryOutcome::Changed)
    } else if round.finished() {
        Some(RecoveryOutcome::Exhausted)
    } else {
        None
    };
    if let Some(outcome) = outcome {
        finish_attempt(
            control,
            &attempt,
            attempt.report(outcome, completed - previous, next),
        )?;
    } else {
        attempt.lock()?.observed = next;
    }
    Ok(())
}

fn pressure_tick(
    peer: &Peer,
    control: &Control,
    pressure: &mut Pressure,
    deadline: &mut Option<Instant>,
    joined: &mut Option<(u64, Instant)>,
) -> Result<()> {
    pressure.expire_process_signals();
    let table = peer.snapshot()?;
    let shared = table
        .rows
        .iter()
        .find(|row| row.id == peer.id())
        .is_some_and(|row| row.sharing == ledger::Sharing::Shared);
    let (owned, busy) = {
        let state = control.lock()?;
        (
            state.pressure_round,
            !state.demands.is_empty() || !state.withdrawals.is_empty() || state.clear_want,
        )
    };
    if let Some((generation, limit)) = *joined {
        if let Some(round) = table
            .round
            .as_ref()
            .filter(|round| round.generation == generation)
        {
            pressure.facts.completed = round.completed()?;
        }
        let finished = table
            .round
            .as_ref()
            .is_none_or(|round| round.generation != generation || round.finished());
        let expired = Instant::now() >= limit;
        if finished || expired || !shared || busy {
            // Only the requesting owner can withdraw this generation. A local pressure
            // observer can stop waiting, never cancel another process's allocation round.
            pressure.facts.budget_exhausted += u64::from(expired && !finished);
            *joined = None;
        }
    }
    if let Some(round) = owned {
        if let Some(value) = table
            .round
            .as_ref()
            .filter(|value| value.generation == round)
        {
            pressure.facts.completed = value.completed()?;
        }
        let finished = table
            .round
            .as_ref()
            .is_none_or(|value| value.generation != round || value.finished());
        let expired = deadline.is_some_and(|limit| Instant::now() >= limit);
        if finished || expired || !shared || busy {
            let mut state = control.lock()?;
            state.pressure_round = None;
            queue_withdrawal(&mut state, round);
            pressure.facts.budget_exhausted += u64::from(expired && !finished);
            state.facts.pressure = pressure.facts.clone();
            *deadline = None;
            control.changed.notify_all();
            return Ok(());
        }
    }
    if !shared || busy {
        return Ok(());
    }
    let sample = pressure.observe(peer)?;
    pressure.expire_process_signals();
    let wanted = pressure.wanted();
    let all_idle = !pressure.facts.new_processes.is_empty();
    pressure.facts.suppressed = crate::working::peer::pressure(
        peer.device,
        wanted != 0
            || !pressure.facts.new_processes.is_empty()
            || owned.is_some()
            || joined.is_some(),
        owned.is_none() && joined.is_none() && pressure.calm(),
    )?;
    if wanted != 0 && owned.is_none() && joined.is_none() {
        let episode_deadline = pressure.signal_deadline()
            .unwrap_or_else(|| Instant::now() + COORDINATION_BUDGET);
        let round = if all_idle { peer.ask_idle(sample.generation)? }
            else { peer.ask_device(sample.generation, wanted)? };
        if round.kind == ledger::AskKind::AllIdle { pressure.consume_process_signals(); }
        let mut state = control.lock()?;
        pressure.facts.round = Some(round.generation);
        pressure.facts.completed = round.completed()?;
        if round.requester == peer.id() {
            if state.demands.is_empty()
                && state.withdrawals.is_empty()
                && !state.clear_want
                && !state.stopping
            {
                state.pressure_round = Some(round.generation);
                *deadline = Some(episode_deadline);
            } else {
                // An allocator may arrive while ask_device is outside Control. Retire this
                // exact pressure generation before that caller starts its own ask.
                queue_withdrawal(&mut state, round.generation);
            }
        } else {
            // Joining another owner never gives us cancellation authority. Clearing our
            // advisory want is also performed before a subsequent allocator asks.
            state.clear_want = true;
            *joined = Some((round.generation, episode_deadline));
        }
        control.changed.notify_all();
    } else if wanted == 0 && !pressure.facts.new_processes.is_empty() {
        // No completed idle pages were present at this signal. Do not make future
        // newly idle pages an unbounded obligation to a steady foreign process.
        pressure.consume_process_signals();
    }
    control.lock()?.facts.pressure = pressure.facts.clone();
    Ok(())
}

fn serve(peer: &Peer, control: &Control, pressure: &mut Pressure) -> Result<()> {
    let mut generation = u64::MAX;
    let mut pressure_deadline = None;
    let mut pressure_joined = None;
    let mut next_sample = Instant::now();
    loop {
        let (used, commands) = {
            let state = control.lock()?;
            let (state, _) = control
                .changed
                .wait_timeout_while(state, CADENCE, |s| {
                    !s.stopping && s.generation == generation && s.commands.is_empty()
                })
                .map_err(|_| crate::Error::Poisoned("native owner wait mutex".into()))?;
            if state.stopping {
                return Ok(());
            }
            generation = state.generation;
            (state.last_use, state.commands.len())
        };
        for _ in 0..commands {
            // Leave unadmitted commands in Control so stop cancels all queued callers.
            let next = {
                let mut state = control.lock()?;
                if state.stopping {
                    return Ok(());
                }
                state.commands.pop_front()
            };
            match next {
                Some(value) => command(peer, control, pressure, value),
                None => break,
            }
        }
        match withdraw_tick(peer, control) {
            Err(error) if deferred(&error) => {
                control.lock()?.facts.deferred += 1;
            }
            result => result?,
        }
        match tick(peer, control, used) {
            Err(error) if deferred(&error) => {
                control.lock()?.facts.deferred += 1;
            }
            result => result?,
        }
        // Donor work above always runs, even while this process is also asking for memory.
        match recover_tick(peer, control) {
            Err(error) if deferred(&error) => {
                control.lock()?.facts.deferred += 1;
            }
            result => result?,
        }
        if Instant::now() >= next_sample {
            next_sample = Instant::now() + CADENCE;
            match pressure_tick(
                peer,
                control,
                pressure,
                &mut pressure_deadline,
                &mut pressure_joined,
            ) {
                Err(error) if deferred(&error) => {
                    pressure.invalidate_observation();
                    let mut state = control.lock()?;
                    state.facts.deferred += 1;
                    state.facts.pressure = pressure.facts.clone();
                }
                result => result?,
            }
        }
    }
}

fn run(mut peer: Peer, control: Arc<Control>, descriptor: ledger::Device) {
    let mut pressure = Pressure::new(descriptor);
    // Keep Peer outside the unwind scope: even an unexpected worker panic retires it safely.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        serve(&peer, &control, &mut pressure)
    }));
    let error = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error.to_string()),
        Err(_) => Some("native owner worker panicked".into()),
    };
    if let Ok(mut state) = control.lock() {
        state.facts.failed = error;
    }
    control.stop();
    let retired = loop {
        match peer.retire() {
            Ok(retired) => {
                drop(retired);
                break true;
            }
            Err((owner, error)) => {
                peer = owner;
                if deferred(&error) {
                    std::thread::sleep(CADENCE);
                } else {
                    // Unknown native ownership/fault state cannot authorize destruction.
                    if let Ok(mut state) = control.lock() {
                        state.facts.failed = Some(error.to_string());
                    }
                    std::mem::forget(peer);
                    break false;
                }
            }
        }
    };
    if let Ok(mut state) = control.lock() {
        state.retired = retired;
        control.changed.notify_all();
    }
}
