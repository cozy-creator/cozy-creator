//! Native donor service. Only this thread owns Peer and its retained GPU resources.
//! Allocator lookup references contain host metadata only, never a GPU destructor or join.
use super::{Peer, Result, Service, deferred};
use crate::ledger;
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak, mpsc};
use std::time::Duration;

const CADENCE: Duration = Duration::from_millis(500);

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
}

enum Command {
    Configure(ledger::Sharing, i32, mpsc::SyncSender<Result<()>>),
    Snapshot(mpsc::SyncSender<Result<ledger::Snapshot>>),
    CudaCalls(mpsc::SyncSender<Result<u64>>),
}

#[derive(Default)]
struct State {
    stopping: bool,
    retired: bool,
    generation: u64,
    last_use: u64,
    commands: VecDeque<Command>,
    facts: Facts,
}

// No Peer, Arena, Driver, Python reference or JoinHandle may be stored here.
struct Control {
    pid: u32,
    state: Mutex<State>,
    changed: Condvar,
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
                        run(peer, worker);
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
        Ok(facts)
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

fn command(peer: &Peer, command: Command) {
    match command {
        Command::Configure(sharing, priority, reply) => {
            let _ = reply.send(peer.configure(sharing, priority));
        }
        Command::Snapshot(reply) => {
            let _ = reply.send(peer.snapshot());
        }
        Command::CudaCalls(reply) => {
            let _ = reply.send(peer.cuda_calls());
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

fn serve(peer: &Peer, control: &Control) -> Result<()> {
    let mut generation = u64::MAX;
    loop {
        let used = {
            let state = control.lock()?;
            let (mut state, _) = control
                .changed
                .wait_timeout_while(state, CADENCE, |s| {
                    !s.stopping && s.generation == generation && s.commands.is_empty()
                })
                .map_err(|_| crate::Error::Poisoned("native owner wait mutex".into()))?;
            if state.stopping {
                return Ok(());
            }
            generation = state.generation;
            state.last_use
        };
        loop {
            // Leave unadmitted commands in Control so stop cancels all queued callers.
            let next = {
                let mut state = control.lock()?;
                if state.stopping {
                    return Ok(());
                }
                state.commands.pop_front()
            };
            match next {
                Some(value) => command(peer, value),
                None => break,
            }
        }
        match tick(peer, control, used) {
            Err(error) if deferred(&error) => {
                control.lock()?.facts.deferred += 1;
            }
            result => result?,
        }
    }
}

fn run(mut peer: Peer, control: Arc<Control>) {
    // Keep Peer outside the unwind scope: even an unexpected worker panic retires it safely.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| serve(&peer, &control)));
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
