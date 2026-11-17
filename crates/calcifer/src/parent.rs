//! `cozy calcifer`: Calcifer's stable parent (cl-1113; Paul, 11-16: "it should be overlapping (v1 → v2) so I never have downtime").
//! The process a service manager (systemd, launchd, the Windows starter) or a command started, and the one it keeps tracking. It holds
//! this home's lock, the loopback listeners, the bearer and the holder record for good, and runs Calcifer itself as a generation: a
//! child started from a cozy binary, handed the listeners. A swap starts the installed cozy as the next generation beside the serving
//! one; once it's prepared the serving one lets go of the home and the next takes it. The listeners never close, so a connect made
//! meanwhile waits in their backlog: nothing is refused, and nothing a client holds (address, bearer) changes.
//!
//! A generation that dies is started again; two that die before answering end the parent with a failure, and its service manager's
//! policy takes over. A next generation that never prepares is ended, the binary it came from is replaced by `.cozy-previous` when
//! that is the serving generation's own, and the serving one keeps serving.
use std::{
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::mpsc,
    time::{Duration, Instant},
};

use ::api::{
    calcifer::{Holder, REFUSED},
    config::Loaded,
    error::{Error, Kind},
};
use platform::{generation, ipc::LocalStream, lock::Held, paths::Home};
use tracing::{error, info, warn};

use crate::generation::{PROTOCOL, ToGeneration, ToParent, receive, send};

/// How long a next generation has to say it's prepared: started, its configuration read, its runtime up.
const PREPARE: Duration = Duration::from_secs(10);
/// How long the first generation has (a cold disk).
const FIRST: Duration = Duration::from_secs(60);

enum Event {
    Said(u64, ToParent),
    /// Its stream ended: it exited, or is exiting.
    Gone(u64),
    /// A service manager's stop (Unix).
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Started; owes `Prepared`.
    Starting,
    /// Prepared as the next generation; the serving one is letting go.
    Prepared,
    /// Told to take the home; owes `Ready`.
    TakingOver,
    /// Answers requests.
    Serving,
    /// Told to hand over; owes `Released`.
    HandingOver,
    /// Let go of the home; told to exit, or will be.
    Released,
    /// Stopping for good (`Down` said, or told to stop).
    Stopping,
}

struct Generation {
    id: u64,
    child: generation::Child,
    control: LocalStream,
    state: State,
    /// What it's started from.
    binary: PathBuf,
    version: Option<String>,
    /// When it must have said `Prepared` by.
    due: Option<Instant>,
}

struct Parent {
    home: Home,
    held: Held,
    listeners: Vec<TcpListener>,
    address: SocketAddr,
    token: String,
    generations: Vec<Generation>,
    next_id: u64,
    events: mpsc::Sender<Event>,
    /// Generations that died before answering, in a row.
    early_deaths: u32,
    /// Stopping for good: no generation is started again.
    ending: bool,
}

/// The parent's whole life: `loaded`'s home is held (`held`) and `listeners` bound.
pub(crate) fn run(loaded: &Loaded, held: Held, listeners: Vec<TcpListener>, address: SocketAddr) -> ExitCode {
    #[cfg(unix)]
    let signals = platform::process::SupervisorSignals::new();
    // Before any thread exists: the signals wait in one thread for every one (generations unblock them again).
    #[cfg(unix)]
    if let Err(e) = signals.block() {
        error!("Calcifer's parent can't take its stop signals: {e}");
        return ExitCode::FAILURE;
    }
    let (events, heard) = mpsc::channel();
    #[cfg(unix)]
    {
        let stops = events.clone();
        let _ = std::thread::Builder::new().name("calcifer-signals".into()).spawn(move || {
            while let Ok(woken) = signals.wait() {
                if matches!(woken, platform::process::Woken::Stop(_)) && stops.send(Event::Stop).is_err() {
                    return;
                }
            }
        });
    }
    let mut token = [0; 32];
    getrandom::fill(&mut token).expect("the OS provides randomness");
    let mut parent = Parent {
        home: loaded.home.clone(),
        held,
        listeners,
        address,
        token: hex::encode(token),
        generations: vec![],
        next_id: 0,
        events,
        early_deaths: 0,
        ending: false,
    };
    // The first is this binary: its parent's own version, whatever is installed.
    let first = std::env::current_exe().unwrap_or_else(|_| crate::cozy::installed(&parent.home).unwrap_or_default());
    if let Err(e) = parent.start(&first, FIRST) {
        error!("starting Calcifer: {e}");
        return ExitCode::FAILURE;
    }
    parent.serve(heard)
}

impl Parent {
    fn start(&mut self, binary: &Path, within: Duration) -> std::io::Result<u64> {
        let listeners: Vec<&TcpListener> = self.listeners.iter().collect();
        // Unix: its output is the parent's own (a service's journal). Windows: the parent's log, which detach gave it.
        let log = cfg!(windows).then(|| self.home.calcifer_log());
        let args = ["calcifer".into(), "generation".into()];
        let (child, control) = generation::start(binary, &args, log.as_deref(), &listeners).map_err(std::io::Error::other)?;
        let id = self.next_id;
        self.next_id += 1;
        let reader = control.try_clone()?;
        let events = self.events.clone();
        std::thread::Builder::new().name(format!("calcifer-generation-{id}")).spawn(move || {
            loop {
                match receive::<ToParent>(&reader) {
                    Ok(message) => {
                        if events.send(Event::Said(id, message)).is_err() {
                            return;
                        }
                    }
                    Err(_) => {
                        let _ = events.send(Event::Gone(id));
                        return;
                    }
                }
            }
        })?;
        send(&control, &ToGeneration::Start { protocol: PROTOCOL, token: self.token.clone(), address: self.address })?;
        info!(pid = child.pid(), binary = %binary.display(), "Calcifer generation started");
        self.generations.push(Generation {
            id,
            child,
            control,
            state: State::Starting,
            binary: binary.to_owned(),
            version: None,
            due: Some(Instant::now() + within),
        });
        Ok(id)
    }

    fn serve(mut self, heard: mpsc::Receiver<Event>) -> ExitCode {
        loop {
            if self.generations.is_empty() {
                return match self.ending {
                    true => ExitCode::SUCCESS,
                    false => ExitCode::FAILURE,
                };
            }
            let due = self.generations.iter().filter_map(|generation| generation.due).min();
            let event = match due {
                Some(due) => match heard.recv_timeout(due.saturating_duration_since(Instant::now())) {
                    Ok(event) => Some(event),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return ExitCode::FAILURE,
                },
                None => match heard.recv() {
                    Ok(event) => Some(event),
                    Err(_) => return ExitCode::FAILURE,
                },
            };
            match event {
                Some(Event::Said(id, message)) => self.said(id, message),
                Some(Event::Gone(id)) => self.gone(id),
                Some(Event::Stop) => self.stop(),
                None => self.overdue(),
            }
        }
    }

    fn find(&mut self, id: u64) -> Option<&mut Generation> {
        self.generations.iter_mut().find(|generation| generation.id == id)
    }

    fn with(&self, state: State) -> Option<u64> {
        self.generations.iter().find(|generation| generation.state == state).map(|generation| generation.id)
    }

    fn tell(&mut self, id: u64, message: &ToGeneration) {
        if let Some(generation) = self.find(id)
            && let Err(e) = send(&generation.control, message)
        {
            warn!(pid = generation.child.pid(), "telling a Calcifer generation: {e}");
        }
    }

    fn said(&mut self, id: u64, message: ToParent) {
        let Some(state) = self.find(id).map(|generation| generation.state) else { return };
        match message {
            ToParent::Prepared { protocol, version } => {
                if let Some(generation) = self.find(id) {
                    generation.version = Some(version.clone());
                    generation.due = None;
                }
                if protocol != PROTOCOL {
                    self.failed(id, &format!("cozy {version} needs a newer Calcifer parent: run `cozy down`, then any cozy command"));
                    return;
                }
                match self.with(State::Serving).or(self.with(State::TakingOver)) {
                    // The first, or one after a death: nothing to take over from.
                    None => self.take_over(id, vec![]),
                    Some(serving) => {
                        if let Some(generation) = self.find(id) {
                            generation.state = State::Prepared;
                        }
                        if let Some(generation) = self.find(serving) {
                            generation.state = State::HandingOver;
                        }
                        self.tell(serving, &ToGeneration::HandOver { to: version });
                    }
                }
            }
            ToParent::Ready { pid, version, started_at_ms } => {
                let holder = Holder { address: self.address, token: self.token.clone(), pid, version: version.clone(), started_at_ms };
                let mut record = serde_json::to_vec(&holder).expect("serializes");
                record.push(b'\n');
                if let Err(e) = self.held.publish(&record) {
                    error!("publishing Calcifer's record: {e}");
                }
                let _ = std::fs::remove_file(self.home.calcifer().join(REFUSED));
                self.early_deaths = 0;
                if let Some(generation) = self.find(id) {
                    generation.state = State::Serving;
                }
                self.tell(id, &ToGeneration::Accept);
                info!(pid, %version, "Calcifer serves");
                if let Some(released) = self.with(State::Released) {
                    self.tell(released, &ToGeneration::Exit);
                }
            }
            ToParent::Swap => {
                if self.ending || self.generations.len() > 1 {
                    self.tell(id, &ToGeneration::SwapFailed { why: "a restart is already under way".into() });
                    return;
                }
                let binary = match crate::cozy::installed(&self.home) {
                    Ok(binary) => binary,
                    Err(e) => return self.tell(id, &ToGeneration::SwapFailed { why: e.message }),
                };
                if let Err(e) = self.start(&binary, PREPARE) {
                    self.tell(id, &ToGeneration::SwapFailed { why: format!("cozy at {} didn't start: {e}", binary.display()) });
                }
            }
            ToParent::Released { views } => {
                if let Some(generation) = self.find(id) {
                    generation.state = State::Released;
                }
                match self.with(State::Prepared) {
                    Some(next) => self.take_over(next, views),
                    // The next one died meanwhile: this one exits, and the binary it came from serves again.
                    None => {
                        self.tell(id, &ToGeneration::Exit);
                        self.again();
                    }
                }
            }
            ToParent::Down => {
                if state != State::Serving && state != State::Stopping {
                    return;
                }
                self.ending = true;
                if let Some(generation) = self.find(id) {
                    generation.state = State::Stopping;
                }
                // A next generation being prepared has nothing to take.
                let preparing: Vec<u64> = self.generations.iter().filter(|g| g.id != id).map(|g| g.id).collect();
                for other in preparing {
                    self.end(other);
                }
            }
        }
    }

    fn take_over(&mut self, id: u64, views: Vec<crate::project::vault::Carried>) {
        if let Some(generation) = self.find(id) {
            generation.state = State::TakingOver;
        }
        self.tell(id, &ToGeneration::TakeOver { views });
    }

    /// A next generation that won't serve: ended, its binary put back when the serving one's is beside it, and the serving one told.
    fn failed(&mut self, id: u64, why: &str) {
        warn!("the next Calcifer didn't start: {why}");
        self.end(id);
        let serving = self.with(State::Serving).or(self.with(State::HandingOver));
        let version = serving.and_then(|serving| self.generations.iter().find(|g| g.id == serving)).and_then(|g| g.version.clone());
        let put_back = version.as_deref().is_some_and(|version| put_back(&self.home, version));
        if let Some(serving) = serving {
            if let Some(generation) = self.find(serving) {
                generation.state = State::Serving;
            }
            let why = match put_back {
                true => format!("{why}; cozy is back on the binary it ran"),
                false => why.to_owned(),
            };
            self.tell(serving, &ToGeneration::SwapFailed { why });
        }
    }

    fn end(&mut self, id: u64) {
        if let Some(generation) = self.find(id) {
            let _ = generation.child.kill();
            let _ = generation.child.wait();
        }
        self.generations.retain(|generation| generation.id != id);
    }

    fn gone(&mut self, id: u64) {
        let Some(at) = self.generations.iter().position(|generation| generation.id == id) else { return };
        let mut generation = self.generations.remove(at);
        let status = generation.child.wait();
        let state = generation.state;
        match state {
            State::Released | State::Stopping => info!(pid = generation.child.pid(), "a Calcifer generation ended"),
            _ => warn!(pid = generation.child.pid(), ?state, ?status, "a Calcifer generation ended"),
        }
        match state {
            State::Released | State::Stopping => {}
            // A next generation that never prepared: the serving one keeps serving.
            State::Starting if self.with(State::Serving).is_some() => {
                self.generations.push(generation);
                self.failed(id, "it exited before it was prepared");
            }
            State::Prepared => {
                self.generations.push(generation);
                self.failed(id, "it exited before it took over");
            }
            // It took the home, or held it: Calcifer starts again.
            State::Starting | State::TakingOver | State::Serving | State::HandingOver => {
                if matches!(state, State::Starting | State::TakingOver) {
                    self.early_deaths += 1;
                }
                if state == State::TakingOver
                    && let Some(released) = self.with(State::Released)
                {
                    let version = self.generations.iter().find(|g| g.id == released).and_then(|g| g.version.clone());
                    if let Some(version) = version {
                        put_back(&self.home, &version);
                    }
                }
                if state == State::HandingOver
                    && let Some(next) = self.with(State::Prepared)
                {
                    // The serving one died handing over: the prepared one takes the home as it is.
                    self.take_over(next, vec![]);
                    return;
                }
                self.again();
            }
        }
        let _ = generation.binary;
    }

    /// Calcifer from the installed cozy again, unless this parent is ending or the last two never answered.
    fn again(&mut self) {
        if self.ending || self.with(State::TakingOver).is_some() || self.with(State::Starting).is_some() {
            return;
        }
        if self.early_deaths >= 2 {
            error!("Calcifer ended twice before answering");
            self.ending = true;
            return;
        }
        let binary = crate::cozy::installed(&self.home).or_else(|_| std::env::current_exe().map_err(|e| Error::internal("calcifer_binary", e.to_string())));
        match binary {
            Ok(binary) => {
                if let Err(e) = self.start(&binary, FIRST) {
                    error!("starting Calcifer again: {e}");
                    self.ending = true;
                }
            }
            Err(e) => {
                error!("starting Calcifer again: {}", e.message);
                self.ending = true;
            }
        }
    }

    /// A service stop: the serving generation stops as `down` does; one being prepared is ended.
    fn stop(&mut self) {
        info!("stop: Calcifer stops");
        self.ending = true;
        let ids: Vec<(u64, State)> = self.generations.iter().map(|generation| (generation.id, generation.state)).collect();
        for (id, state) in ids {
            match state {
                State::Serving | State::HandingOver | State::TakingOver => {
                    if let Some(generation) = self.find(id) {
                        generation.state = State::Stopping;
                    }
                    self.tell(id, &ToGeneration::Stop);
                }
                State::Stopping | State::Released => {}
                State::Starting | State::Prepared => self.end(id),
            }
        }
    }

    fn overdue(&mut self) {
        let now = Instant::now();
        let late: Vec<u64> = self.generations.iter().filter(|g| g.due.is_some_and(|due| due <= now)).map(|g| g.id).collect();
        for id in late {
            if self.with(State::Serving).is_some() {
                self.failed(id, "it wasn't prepared in time");
            } else {
                warn!("Calcifer wasn't prepared in time");
                self.end(id);
                self.early_deaths += 1;
                self.again();
            }
        }
    }
}

/// Puts `.cozy-previous` back as the installed cozy when it is `version`, the one that keeps serving (an update that doesn't start).
fn put_back(home: &Home, version: &str) -> bool {
    let Ok(installed) = crate::cozy::installed(home) else { return false };
    let previous = installed.with_file_name(".cozy-previous");
    let says = std::process::Command::new(&previous).arg("--version").output();
    if !says.is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == format!("cozy {version}")) {
        return false;
    }
    match std::fs::rename(&previous, &installed) {
        Ok(()) => {
            info!(binary = %installed.display(), %version, "the update didn't start: cozy is back on the binary it ran");
            true
        }
        Err(e) => {
            warn!("putting {} back: {e}", previous.display());
            false
        }
    }
}

/// Why the parent couldn't start, as `refuse` records it.
pub(crate) fn port_error(port: u16, e: std::io::Error) -> Error {
    Error::new(Kind::Unavailable, "calcifer_port", format!("Calcifer cannot listen on 127.0.0.1:{port}: {e}"))
        .remedy("free the port, or take calcifer.port out of config.toml to use the next free one")
}
