//! This computer's machine, `local`: always recorded, never enabled. Anything that uses it starts
//! its TensorD (`cozy machine serve`) when none runs, detached from Calcifer and from Calcifer's
//! service, so a Calcifer restart never ends it. TensorD exits by itself after 15
//! minutes idle; its executors give their memory back after 5. One start at a time here, and
//! TensorD's lock on its root keeps one machine per root whoever starts it.
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

use platform::{
    lock::{Kind as LockKind, LockFile},
    paths::Home,
    service::{ServiceSpec, Services},
    watch::{Watcher, Writes},
};
use api::{
    config,
    error::{Error, Kind},
};
use tracing::{info, warn};

use api::machine::Booting;

use crate::store::Endpoint;

/// How long this computer's machine may take to serve once started (cl-977), and then how long it may stay on one step of its start
/// (`booting.step`, cl-1066): a start that moves on keeps being waited for (runs never stuck, Paul 11-15). Past it, it isn't
/// answering, and local takes no work until it serves or exits. TensorD's own waits on the way are bounded well below it: 70 s for its
/// root's lock, 60 s for a fresh process to open its GPUs, 60 s per executor stuck in the kernel.
pub(crate) const START: Duration = Duration::from_secs(300);

pub(crate) struct Local {
    root: PathBuf,
    home: Home,
    services: Services,
    pub(crate) starting: tokio::sync::Mutex<()>,
    /// A start past `START` with no answer: its process, when, and the answer it waits for (cl-977).
    silent: Arc<Mutex<Option<Silent>>>,
}

struct Silent {
    pid: u32,
    began: Instant,
    before: Option<SystemTime>,
    /// What was stuck when it was judged silent (`stuck_now`).
    stuck: String,
}

/// How long the look at what's stuck may take: a read on a filesystem that stopped answering waits for good.
const LOOK: Duration = Duration::from_secs(2);

impl Local {
    pub(crate) fn new(home: &Home, user_home: &Path, config: &config::Config) -> Self {
        Self {
            root: config.machine.root(home),
            home: home.clone(),
            services: Services::new(home, user_home),
            starting: tokio::sync::Mutex::new(()),
            silent: Arc::default(),
        }
    }

    fn lock(&self) -> PathBuf {
        self.root.join("var/lib/cozy/machine/agent.lock")
    }

    /// Where TensorD keeps the births of each vault view's processes (cl-1064).
    pub(crate) fn births(&self) -> PathBuf {
        self.root.join("var/lib/cozy/tensord/private-executors")
    }

    fn ready(&self) -> PathBuf {
        self.root.join("var/lib/cozy/tensord/api-ready.json")
    }

    fn start_log(&self) -> PathBuf {
        self.root.join("var/lib/cozy/machine/start.log")
    }

    /// Whether a TensorD holds this computer's machine root now.
    pub(crate) fn running(&self) -> bool {
        held(&self.lock())
    }

    /// Where the running TensorD serves, as its `api-ready.json` says.
    pub(crate) fn endpoint(&self) -> Result<Endpoint, Error> {
        crate::machines::endpoint_of(&self.ready())
    }

    /// Where other computers reach the running TensorD, as its `api-ready.json` says: none on
    /// loopback, or when none runs.
    pub(crate) fn reach(&self) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct Ready {
            reach: Option<String>,
        }
        let ready: Ready = serde_json::from_slice(&std::fs::read(self.ready()).ok()?).ok()?;
        ready.reach.filter(|_| self.running())
    }

    /// Where the running TensorD serves; none when none runs. One that holds the root and hasn't written where yet (another start's)
    /// is waited for at most `START`, by its answer's file.
    pub(crate) async fn serving(&self) -> Result<Option<Endpoint>, Error> {
        if !self.running() {
            return Ok(None);
        }
        if let Ok(endpoint) = self.endpoint() {
            return Ok(Some(endpoint));
        }
        let woke = Arc::new(tokio::sync::Notify::new());
        let watched = woke.clone();
        let _watch = self.watch(move || watched.notify_one())?;
        let began = Instant::now();
        let answered = async {
            loop {
                if let Ok(endpoint) = self.endpoint() {
                    return Some(endpoint);
                }
                woke.notified().await;
            }
        };
        match tokio::time::timeout(START, answered).await {
            Ok(endpoint) => Ok(endpoint),
            Err(_) if !self.running() => Ok(None),
            Err(_) => Err(not_answering(began, &self.stuck(None))),
        }
    }

    /// Wakes `woke` as TensorD writes where it serves.
    fn watch(&self, woke: impl FnMut() + Send + 'static) -> Result<Watcher, Error> {
        let dir = self.ready().parent().expect("a file has a directory").to_owned();
        let failed = |e: String| Error::new(Kind::Internal, "watch_failed", format!("{}: {e}", dir.display()));
        // Before its first start the directory isn't there yet.
        std::fs::create_dir_all(&dir).map_err(|e| failed(e.to_string()))?;
        Watcher::new("local-answer", &[(&dir, Writes::Named(&["api-ready.json"]))], woke).map_err(|e| failed(e.to_string()))
    }

    /// A start that hasn't answered past `START` (cl-977), judged now: still silent, its refusal; once it served or exited, none.
    pub(crate) fn silent(&self) -> Option<Error> {
        let mut silent = self.silent.lock().unwrap();
        let started = silent.as_ref()?;
        let answered = served_at(&self.ready()) > started.before && self.running();
        let exited = platform::process::Birth::of(started.pid).map_or(true, |birth| birth.ended().unwrap_or(true));
        if answered || exited {
            *silent = None;
            return None;
        }
        Some(not_answering(started.began, &started.stuck))
    }

    /// What is stuck now (`stuck_now`), for a start that isn't answering.
    fn stuck(&self, pid: Option<u32>) -> String {
        stuck_now(self.root.clone(), self.start_log(), pid)
    }

    /// Another computer's controller is authorized to use this machine (`cozy machine
    /// authorize`): Calcifer starts it with itself, at login, and it stays up.
    pub(crate) fn authorized(&self) -> bool {
        let text = std::fs::read_to_string(self.home.keys().join("authorized")).unwrap_or_default();
        text.lines().any(|line| !line.split('#').next().unwrap_or_default().trim().is_empty())
    }

    /// Starts TensorD from the installed `cozy` and waits until it serves: its `api-ready.json`
    /// written after the start while it holds its root. A start that exits first is refused with
    /// its log. One whose step hasn't moved for `START` is refused `machine_not_answering` (cl-977), told to `silent`; it is judged
    /// again as it answers (`answered` gets where it serves) or at the next use. It is waited for and judged on a task of its own,
    /// whether or not the caller still waits (cl-1066); one the caller left that serves goes to `answered` too. `key` reads its step.
    pub(crate) async fn start(
        &self,
        key: PathBuf,
        answered: impl FnOnce(Endpoint) + Send + 'static,
        silent: impl FnOnce(Error) + Send + 'static,
    ) -> Result<Endpoint, Error> {
        let log = self.start_log();
        let failed = |why: String| start_failed(&log, why);
        // A pod's machine is its image's, a rental with a Hub and an idle clock. One this starts would
        // own its root as neither.
        if config::image_machine() {
            return Err(failed("on a rental, its image starts the machine, and it isn't running now".into()));
        }
        let program = self.home.launcher().ok_or_else(|| {
            failed(format!("there's no cozy installed in {}", self.home.root().display()))
        })?;
        let dir = log.parent().expect("a log has a directory");
        std::fs::create_dir_all(dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
        let before = served_at(&self.ready());
        let spec = ServiceSpec {
            name: "tensord".into(),
            program,
            args: vec!["machine".into(), "serve".into()],
            log: Some(log.clone()),
        };
        let mut child = self.services.detach(&spec).map_err(|e| failed(e.to_string()))?;
        let pid = child.pid();
        // A health event: Calcifer's own machine start, by its process alone.
        info!(target: "cozy.service_health", pid, "starting this computer's machine");
        let mut wait = Answer {
            lock: self.lock(),
            ready: self.ready(),
            before,
            exited: Default::default(),
            woke: Arc::new(tokio::sync::Notify::new()),
            _watch: None,
        };
        // Calcifer reaps it when it exits, idle or ended: a thread of its own, which Calcifer's exit never waits for, so the machine
        // outlives Calcifer.
        let (exited, woke) = (wait.exited.clone(), wait.woke.clone());
        std::thread::Builder::new()
            .name("reap-tensord".into())
            .spawn(move || {
                let status = child.wait().map_err(|e| e.to_string());
                *exited.lock().unwrap() = Some(status);
                woke.notify_one();
            })
            .map_err(|e| failed(e.to_string()))?;
        let woke = wait.woke.clone();
        wait._watch = Some(self.watch(move || woke.notify_one()).map_err(|e| failed(e.message))?);
        let began = Instant::now();
        let (told, heard) = tokio::sync::oneshot::channel();
        let (judged, root, ready, start_log) = (self.silent.clone(), self.root.clone(), self.ready(), log.clone());
        tokio::spawn(async move {
            // Slow is no hang (runs never stuck, Paul 11-15): it is waited for while its start moves on (`booting.step`, cl-1066), and
            // judged silent once it has stayed on one step for `START`, or its step can't be read. Its process tree's CPU and I/O are no
            // measure: its booting listener answers each look while a step sits in the kernel.
            let mut due = began + START;
            let mut last = None;
            let served = loop {
                match tokio::time::timeout_at(due.into(), wait.served()).await {
                    Ok(served) => break Some(served),
                    Err(_) => match booting(&ready, &key).await {
                        Some(Booting { on_step_ms: Some(ms), .. }) if Duration::from_millis(ms) < START => {
                            due = Instant::now() + START - Duration::from_millis(ms);
                        }
                        // An older machine says only how far it got: moved since the last look, it has another `START`.
                        Some(Booting { on_step_ms: None, progress, .. }) if last.replace(progress).is_none_or(|was| was != progress) => {
                            due = Instant::now() + START;
                        }
                        _ => break None,
                    },
                }
            };
            let result = match served {
                Some(Ok(endpoint)) => {
                    info!(ms = began.elapsed().as_millis() as u64, "this computer's machine serves");
                    Ok(endpoint)
                }
                Some(Err(why)) => Err(start_failed(&start_log, why)),
                None => {
                    let stuck = stuck_now(root, start_log.clone(), Some(pid));
                    let refusal = not_answering(began, &stuck);
                    warn!("{}", refusal.message);
                    *judged.lock().unwrap() = Some(Silent { pid, began, before, stuck });
                    silent(refusal.clone());
                    Err(refusal)
                }
            };
            let late = result.as_ref().is_err_and(|refusal| refusal.code == "machine_not_answering");
            match told.send(result) {
                // Its caller left: where it serves is recorded all the same.
                Err(Ok(endpoint)) => answered(endpoint),
                _ if late => {
                    // Its answer, or its exit, judged as each comes: no deadline now, and nothing read.
                    if let Ok(endpoint) = wait.served().await {
                        info!(s = began.elapsed().as_secs(), "this computer's machine serves, late");
                        answered(endpoint);
                    }
                }
                _ => {}
            }
        });
        heard.await.unwrap_or_else(|_| Err(failed("its start was abandoned".into())))
    }
}

/// `machine_start_failed`: why, and where its log is.
fn start_failed(log: &Path, why: String) -> Error {
    let message = format!("This computer's machine didn't start: {why}. Its log is at {}.", log.display());
    Error::new(Kind::Unavailable, "machine_start_failed", message).next("cozy doctor")
}

/// How far a start has got, as its booting listener answers (`booting`); none when it can't be read within `LOOK`. It answers where
/// the last one served (`api-ready.json`): the same port and certificate, kept with the root.
async fn booting(ready: &Path, key: &Path) -> Option<Booting> {
    let endpoint = crate::machines::endpoint_of(ready).ok()?;
    let link = crate::link::Link::new(&endpoint, crate::machines::signer(key).ok()?).ok()?;
    tokio::time::timeout(LOOK, link.status()).await.ok()?.ok()?.booting
}

/// What a start waits on: TensorD's answer (`api-ready.json`, newer than `before`, while it holds its root) or its exit, each of
/// which wakes `woke`.
struct Answer {
    lock: PathBuf,
    ready: PathBuf,
    before: Option<SystemTime>,
    exited: Arc<Mutex<Option<Result<std::process::ExitStatus, String>>>>,
    woke: Arc<tokio::sync::Notify>,
    _watch: Option<Watcher>,
}

impl Answer {
    async fn served(&mut self) -> Result<Endpoint, String> {
        loop {
            if served_at(&self.ready) > self.before && held(&self.lock) {
                return crate::machines::endpoint_of(&self.ready).map_err(|e| e.message);
            }
            match self.exited.lock().unwrap().as_ref() {
                // Another start holds the root: it serves soon, or lets go.
                Some(Ok(status)) if status.success() && held(&self.lock) => {}
                Some(Ok(status)) => return Err(format!("the machine exited ({status}) before it was ready")),
                Some(Err(why)) => return Err(why.clone()),
                None => {}
            }
            self.woke.notified().await;
        }
    }
}

/// `machine_not_answering`: how long since it started, what is stuck, and the next command.
fn not_answering(began: Instant, stuck: &str) -> Error {
    let message = format!("This computer's TensorD isn't answering: it still isn't ready {}s after it started{stuck}.", began.elapsed().as_secs());
    Error::new(Kind::Unavailable, "machine_not_answering", message)
        .remedy("This computer takes no work until it's ready. Run it on another machine, or rent one.")
        .next("cozy machine logs local")
}

/// What is stuck: an earlier TensorD that can't exit (`stuck.json`), a process of this start's in the kernel, its log's last line.
/// Read on a thread of its own and waited for at most `LOOK`: the files may be on a filesystem that stopped answering.
fn stuck_now(root: PathBuf, log: PathBuf, pid: Option<u32>) -> String {
    let (said, heard) = std::sync::mpsc::channel();
    let looking = std::thread::Builder::new().name("local-stuck".into()).spawn(move || {
        let mut stuck = stuck_before(&root.join("var/lib/cozy/tensord/update/stuck.json"));
        for member in pid.map(platform::process::tree).unwrap_or_default() {
            let pid = member.birth().pid;
            if let Some(wchan) = platform::process::in_kernel(pid) {
                let at = Some(&wchan).filter(|w| !w.is_empty()).map(|w| format!(" at {w}")).unwrap_or_default();
                stuck += &format!("; process {pid} is stuck in the kernel{at}");
            }
        }
        if let Some(line) = std::fs::read_to_string(&log).ok().as_deref().and_then(platform::process::last_line) {
            stuck += &format!("; its log ends with: {line}");
        }
        let _ = said.send(stuck);
    });
    match looking {
        Ok(_) => heard.recv_timeout(LOOK).unwrap_or_else(|_| format!("; its files didn't answer a read within {}s", LOOK.as_secs())),
        Err(_) => String::new(),
    }
}

/// Whether a TensorD holds the machine root whose lock is at `lock`.
fn held(lock: &Path) -> bool {
    lock.exists() && matches!(LockFile::new(lock).try_lock(LockKind::Exclusive), Ok(None))
}

/// The TensorDs before this one that can't exit, as their stable parent recorded them (`tensord::machine::stuck`), each still there.
fn stuck_before(record: &Path) -> String {
    let Ok(raw) = std::fs::read(record) else { return String::new() };
    let Ok(all) = serde_json::from_slice::<Vec<serde_json::Value>>(&raw) else { return String::new() };
    let mut said = String::new();
    for stuck in all {
        let Ok(birth) = serde_json::from_value::<platform::process::Birth>(stuck["birth"].clone()) else { continue };
        let same = platform::process::Birth::of(birth.pid).is_ok_and(|now| now == birth);
        if !same || (platform::process::threads(birth.pid).is_empty() && birth.ended().unwrap_or(true)) {
            continue;
        }
        let thread = stuck["threads"].as_array().and_then(|threads| threads.iter().find(|t| t["state"] == "D"));
        let at = thread.and_then(|t| t["wchan"].as_str()).filter(|w| !w.is_empty()).map(|w| format!(" at {w}")).unwrap_or_default();
        said += &format!("; the previous TensorD (process {}) can't exit and is stuck in the kernel{at}", birth.pid);
    }
    said
}

/// When the machine last wrote that it serves: its `api-ready.json`'s mtime.
fn served_at(ready: &Path) -> Option<SystemTime> {
    std::fs::metadata(ready).and_then(|m| m.modified()).ok()
}

impl Local {
    /// The file of the controllers' keys this machine admits (`cozy machine authorize`).
    pub(crate) fn keys_file(&self) -> PathBuf {
        self.home.keys().join("authorized")
    }

    /// Adds (`add`) or takes back `key` in `keys_file`: true when the file changed. TensorD reads
    /// the file again at its next call.
    pub(crate) fn authorize(&self, key: &str, add: bool) -> Result<bool, Error> {
        valid_key(key)?;
        let file = self.keys_file();
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(keys_error(&file, e)),
        };
        let held = text.lines().any(|line| uncommented(line) == key);
        let text = match (add, held) {
            (true, false) => {
                let mut text = text;
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text + key + "\n"
            }
            (false, true) => {
                let kept: Vec<&str> = text.lines().filter(|line| uncommented(line) != key).collect();
                let mut left = kept.join("\n");
                if !left.is_empty() {
                    left.push('\n');
                }
                left
            }
            _ => return Ok(false),
        };
        let dir = file.parent().expect("a keys file has a directory");
        platform::durable::private_dir(dir).map_err(|e| keys_error(&file, std::io::Error::other(e)))?;
        platform::durable::replace(&file, text.as_bytes(), 0o600).map_err(|e| keys_error(&file, std::io::Error::other(e)))?;
        Ok(true)
    }

    /// It runs as a worker image's machine (its root is `/`), which its renter names the address of.
    pub(crate) fn image(&self) -> bool {
        self.root == Path::new("/")
    }
}

/// A keys file's line without its comment.
fn uncommented(line: &str) -> &str {
    line.split('#').next().unwrap_or_default().trim()
}

fn keys_error(file: &Path, e: std::io::Error) -> Error {
    Error::new(Kind::Internal, "keys_unwritable", format!("{}: {e}", file.display()))
}

/// Refuses anything but a controller's public key: 32 bytes of unpadded base64url Ed25519.
fn valid_key(key: &str) -> Result<(), Error> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let valid = (URL_SAFE_NO_PAD.decode(key).ok())
        .and_then(|raw| <[u8; 32]>::try_from(raw).ok())
        .is_some_and(|raw| ed25519_dalek::VerifyingKey::from_bytes(&raw).is_ok());
    match valid {
        true => Ok(()),
        false => Err(Error::new(
            Kind::Validation,
            "public_key_invalid",
            format!("{key:?} isn't a valid computer key (32 bytes of unpadded base64url)."),
        )
        .next("on that computer: cozy machine show local (its `controller key`)")),
    }
}
