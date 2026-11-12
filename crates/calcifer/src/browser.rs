//! One Cozy-owned private Chrome process. No normal-browser attachment, TCP debugger, credential
//! URL, durable session map, or disk-backed temporary profile is a fallback.
mod profile;
mod proof;
#[cfg(target_os = "linux")]
mod startup;

/// Start only after the native listener/holder is ready; the inherited channel is otherwise inert.
pub(crate) fn hosted_proof(api: Arc<Api>) { proof::start(api); }
/// The exclusive Caf-home holder recovers only its own nonsecret tmpfs ownership receipts.
pub(crate) fn recover_profiles(home: &std::path::Path) -> Result<(), Refusal> { profile::recover(home) }
use std::{collections::HashMap, net::SocketAddr, sync::{Arc, atomic::{AtomicBool, Ordering}}};
use ::api::error::Kind;
use axum::{Json, extract::State, http::HeaderMap};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::{io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader}, sync::{Mutex, OwnedMutexGuard}};
use crate::{api::{self, Api, Refusal}, sessions::{OwnedSessionLease, Sessions}};
use profile::{BrowserRetired, VolatileProfileLease};

pub(crate) fn daemon_birth() -> Result<String, Refusal> {
    let birth = platform::process::Birth::own().map_err(|_| unavailable())?;
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&birth).map_err(|_| unavailable())?)))
}

pub(crate) async fn project_storage(State(api): State<Arc<Api>>, headers: HeaderMap) -> Result<Json<Value>, Refusal> {
    let token = api::bearer(&headers).filter(|token| api.admits(token) || api.sessions.admits_owned_token(token))
        .ok_or_else(|| api::refuse(Kind::Forbidden, "private_browser_required", "Open project data in Cozy's owned private browser or native CLI"))?;
    Ok(Json(json!({ "version": 3, "daemon_birth": daemon_birth()?, "principal": hex::encode(Sha256::digest(token)) })))
}

fn unavailable() -> Refusal { api::refuse(Kind::Unavailable, "private_browser_unavailable", "Cozy could not establish its owned private browser; no default browser was opened") }
fn busy() -> Refusal { api::refuse(Kind::Conflict, "project_busy", "The Cozy browser has not confirmed that all edits are saved; keep it open and finish saving before locking") }

#[derive(Clone, Copy)]
enum Step { Executable, Home, ProfileSetup, Capture, Pipe, Spawn, Birth, Ownership, Context, Target, Attach, Seal, Inject, Navigate, Activate }
impl Step {
    fn code(self) -> &'static str {
        match self {
            Self::Executable => "browser_executable", Self::Home => "browser_home", Self::ProfileSetup => "browser_profile_setup",
            Self::Capture => "browser_capture", Self::Pipe => "browser_pipe", Self::Spawn => "browser_spawn", Self::Birth => "browser_birth",
            Self::Ownership => "browser_ownership", Self::Context => "browser_context", Self::Target => "browser_target",
            Self::Attach => "browser_attach", Self::Seal => "browser_seal", Self::Inject => "browser_inject",
            Self::Navigate => "browser_navigate", Self::Activate => "browser_activate",
        }
    }
    fn failed(self, mut error: Refusal) -> Refusal { error.0.cause = Some(self.code().into()); error }
    fn unavailable(self) -> Refusal { self.failed(unavailable()) }
}

struct Pipe { birth: platform::process::Birth, send: tokio::fs::File, receive: BufReader<tokio::fs::File>, next: u64 }
impl Pipe {
    async fn call(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<Value, Refusal> {
        self.next += 1;
        let id = self.next;
        let mut request = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session { request["sessionId"] = json!(session); }
        let mut bytes = serde_json::to_vec(&request).map_err(|_| unavailable())?;
        bytes.push(0);
        self.send.write_all(&bytes).await.map_err(|_| unavailable())?;
        self.send.flush().await.map_err(|_| unavailable())?;
        loop {
            let mut bytes = Vec::new();
            let gone = async {
                loop {
                    if self.birth.ended().unwrap_or(false) { return; }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            };
            let length = tokio::select! {
                result = self.receive.read_until(0, &mut bytes) => result.map_err(|_| unavailable())?,
                () = gone => return Err(unavailable()),
            };
            if length == 0 { return Err(unavailable()); }
            if bytes.len() > 8 << 20 { return Err(unavailable()); }
            bytes.pop();
            let mut answer: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
            if answer["id"] != id { continue; }
            if let Some(error) = answer.get("error") {
                let mut refused = unavailable();
                if let Some(code) = error["code"].as_i64().filter(|code| matches!(*code, -32700 | -32600 | -32601 | -32602 | -32603 | -32000)) {
                    refused.0.metadata = Some(json!({ "browser_protocol_code": code }));
                }
                return Err(refused);
            }
            return Ok(answer["result"].take());
        }
    }
}

struct Live {
    tree: platform::process::Tree,
    birth: platform::process::Birth,
    profile: VolatileProfileLease,
    authority: OwnedSessionLease,
    credential: ::api::calcifer::BrowserSession,
    address: SocketAddr,
    pipe: Pipe,
    context: String,
    targets: HashMap<String, String>,
    #[cfg(target_os = "linux")]
    startup: Option<startup::Capture>,
}

impl Live {
    fn failed(&self, error: Refusal, step: Step) -> Refusal {
        let mut error = step.failed(error);
        #[cfg(target_os = "linux")]
        if let Some(capture) = &self.startup {
            let mut facts = error.0.metadata.take().and_then(|value| value.as_object().cloned()).unwrap_or_default();
            facts.extend(capture.facts());
            facts.insert("browser_process_exited".into(), json!(self.birth.ended().unwrap_or(false)));
            error.0.metadata = Some(Value::Object(facts));
        }
        error
    }

    fn seal_startup(&mut self) -> Result<(), Refusal> {
        #[cfg(target_os = "linux")]
        if let Some(capture) = &self.startup {
            capture.seal().map_err(|_| self.failed(unavailable(), Step::Seal))?;
            self.startup.take();
        }
        Ok(())
    }
}

pub(crate) struct OwnedBrowser { live: Arc<Mutex<Option<Live>>>, retirement_incomplete: Arc<AtomicBool> }
/// A one-use frozen-session receipt. Holding it prevents opens/replacements until exact retirement.
pub(crate) struct PreparedBrowserClose {
    owner: Arc<Mutex<Option<Live>>>,
    live: Option<OwnedMutexGuard<Option<Live>>>,
    pages: Vec<String>,
}

impl Drop for PreparedBrowserClose {
    fn drop(&mut self) {
        let Some(held) = self.live.take() else { return };
        let exact = held.as_ref().map(|live| (live.birth.clone(), live.authority.id().to_owned(), live.profile.identity()));
        drop(held);
        let Some((birth, session, profile)) = exact else { return };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let (owner, pages) = (self.owner.clone(), std::mem::take(&mut self.pages));
        runtime.spawn(async move {
            let mut held = owner.lock().await;
            let Some(live) = held.as_mut().filter(|live| live.birth == birth && live.authority.id() == session && live.profile.identity() == profile) else { return };
            // A later vault/run drain may refuse after browser preparation. Abandoning that
            // one-use receipt restores only this exact saved session, never a replacement.
            for page in pages {
                let _ = live.pipe.call("Runtime.evaluate", json!({ "expression": "globalThis.__cozyResume?.()", "returnByValue": true }), Some(&page)).await;
            }
        });
    }
}

impl OwnedBrowser {
    pub(crate) fn new() -> Self { Self { live: Arc::default(), retirement_incomplete: Arc::new(AtomicBool::new(false)) } }

    #[cfg(target_os = "linux")]
    async fn launch(sessions: &Sessions, address: SocketAddr) -> Result<Live, Refusal> {
        use std::process::{Command, Stdio};
        let path = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>();
        let program = ["google-chrome", "chromium", "chromium-browser"].into_iter().find_map(|name| platform::tools::on_path(name, &path)).ok_or_else(|| Step::Executable.unavailable())?;
        let profile = VolatileProfileLease::create()?;
        let mut command = Command::new(program.path);
        let original_home = std::env::home_dir().ok_or_else(|| Step::Home.unavailable())?;
        let original_runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
        let original_authority = std::env::var_os("XAUTHORITY").map(std::path::PathBuf::from)
            .or_else(|| std::env::home_dir().map(|home| home.join(".Xauthority")));
        for directory in ["tmp", "cache", "config", "data", "state", "runtime"] {
            std::fs::create_dir(profile.path().join(directory)).map_err(|_| Step::ProfileSetup.unavailable())?;
        }
        // A deliberate Save must choose an external destination, rather than silently landing
        // in the volatile profile that a later project lock removes.
        let defaults = profile.path().join("Default");
        std::fs::create_dir(&defaults).map_err(|_| Step::ProfileSetup.unavailable())?;
        let preferences = serde_json::to_vec(&json!({ "download": {
            "prompt_for_download": true, "default_directory": original_home.join("Downloads"), "directory_upgrade": true
        }, "session": { "restore_on_startup": 5 } })).map_err(|_| unavailable())?;
        platform::durable::replace(&defaults.join("Preferences"), &preferences, 0o600).map_err(|_| Step::ProfileSetup.unavailable())?;
        if let Some(authority) = original_authority.filter(|path| path.is_absolute() && path.is_file()) { command.env("XAUTHORITY", authority); }
        if let (Some(runtime), Some(display)) = (original_runtime, std::env::var_os("WAYLAND_DISPLAY")) {
            let display = std::path::PathBuf::from(display);
            command.env("WAYLAND_DISPLAY", if display.is_absolute() { display } else { runtime.join(display) });
        }
        command.args(["--incognito", "--remote-debugging-pipe", "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--disable-background-networking", "--disable-sync", "--disable-component-update", "--disable-crash-reporter", "--disable-breakpad", "--disable-crashpad", "--noerrdialogs", "--no-startup-window", "--disable-session-crashed-bubble"])
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg(format!("--disk-cache-dir={}", profile.path().join("cache").display()))
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .env("HOME", profile.path()).env("TMPDIR", profile.path().join("tmp"))
            .env("TMP", profile.path().join("tmp")).env("TEMP", profile.path().join("tmp"))
            .env("XDG_CACHE_HOME", profile.path().join("cache")).env("XDG_CONFIG_HOME", profile.path().join("config"))
            .env("XDG_DATA_HOME", profile.path().join("data")).env("XDG_STATE_HOME", profile.path().join("state"))
            .env("XDG_RUNTIME_DIR", profile.path().join("runtime"));
        let capture = startup::Capture::create().map_err(|_| Step::Capture.unavailable())?;
        command.stderr(capture.stdio().map_err(|_| Step::Capture.unavailable())?);
        let (send, receive) = platform::process::private_browser_pipe(&mut command).map_err(|_| Step::Pipe.unavailable())?;
        let tree = platform::process::spawn(command).map_err(|_| Step::Spawn.unavailable())?;
        let birth = platform::process::Birth::of(tree.pid()).map_err(|_| Step::Birth.unavailable())?;
        let (authority, credential) = sessions.begin_owned();
        let mut live = Live { tree, birth: birth.clone(), profile, authority, credential, address,
            pipe: Pipe { birth, send: tokio::fs::File::from_std(send), receive: BufReader::new(tokio::fs::File::from_std(receive)), next: 0 },
            context: String::new(), targets: HashMap::new(), startup: Some(capture) };
        if let Err(error) = live.profile.observe(&live.birth) { return Err(Self::launch_failed(live, error, Step::Ownership).await); }
        let context = match live.pipe.call("Target.createBrowserContext", json!({ "disposeOnDetach": true }), None).await {
            Ok(context) => context,
            Err(error) => return Err(Self::launch_failed(live, error, Step::Context).await),
        };
        let Some(context) = context["browserContextId"].as_str() else { return Err(Self::launch_failed(live, unavailable(), Step::Context).await) };
        live.context = context.into();
        Ok(live)
    }

    #[cfg(target_os = "linux")]
    async fn launch_failed(mut live: Live, error: Refusal, step: Step) -> Refusal {
        // No page or credential was installed. Keep the real pre-cleanup exit status and
        // closed startup observations, then retire only this newly owned process/profile.
        let error = live.failed(error, step);
        tokio::task::spawn_blocking(move || {
            use std::os::unix::process::ExitStatusExt as _;
            let mut error = error;
            let processes = Self::processes(&live.birth);
            let already_exited = live.birth.ended().unwrap_or(false);
            let killed = already_exited || live.tree.kill().is_ok();
            let status = killed.then(|| live.tree.wait().ok()).flatten();
            let ended = killed && platform::process::end_tree(&processes).is_ok()
                && platform::process::group_members(&live.birth).is_empty();
            let mut facts = error.0.metadata.take().and_then(|value| value.as_object().cloned()).unwrap_or_default();
            facts.insert("browser_exit_before_retirement".into(), json!(already_exited));
            if let Some(status) = status {
                facts.insert("browser_exit_code".into(), json!(status.code()));
                facts.insert("browser_exit_signal".into(), json!(status.signal()));
            }
            let retired = if ended {
                let proof = BrowserRetired { profile: live.profile.identity(), birth: live.birth.clone(), session: live.profile.nonce().to_owned() };
                drop(live.authority);
                live.profile.retire(proof).is_ok()
            } else { false };
            facts.insert("browser_startup_retired".into(), json!(retired));
            error.0.metadata = Some(Value::Object(facts));
            error
        }).await.unwrap_or_else(|_| step.unavailable())
    }
    #[cfg(not(target_os = "linux"))]
    async fn launch(_sessions: &Sessions, _address: SocketAddr) -> Result<Live, Refusal> { Err(unavailable()) }

    /// `project_ref` is an index reference, resolved by the native-only route without opening project.db.
    pub(crate) async fn open(&self, sessions: &Sessions, address: SocketAddr, project_ref: &str) -> Result<(), Refusal> {
        // A prepared close owns this mutex while shutdown drains mutation leases.
        // Never wait here while a browser-open request itself owns such a lease.
        let mut held = self.live.try_lock().map_err(|_| busy())?;
        if self.retirement_incomplete.load(Ordering::Acquire) { return Err(unavailable()); }
        if held.is_none() {
            self.retirement_incomplete.store(true, Ordering::Release);
            match Self::launch(sessions, address).await {
                Ok(live) => *held = Some(live),
                Err(error) => {
                    if error.0.metadata.as_ref().is_some_and(|facts| facts["browser_startup_retired"] == true) { self.retirement_incomplete.store(false, Ordering::Release); }
                    return Err(error);
                }
            }
            self.retirement_incomplete.store(false, Ordering::Release);
            self.watch_windows();
        }
        let live = held.as_mut().ok_or_else(unavailable)?;
        if live.address != address || live.birth.ended().map_err(|_| unavailable())? {
            return Err(api::refuse(Kind::Conflict, "private_browser_retirement_incomplete", "The previous Cozy browser must be retired before another session can open"));
        }
        if let Some(target) = live.targets.get(project_ref) {
            live.pipe.call("Target.activateTarget", json!({ "targetId": target }), None).await.map_err(|error| live.failed(error, Step::Activate))?;
            return Ok(());
        }
        let created = live.pipe.call("Target.createTarget", json!({ "url": "about:blank", "browserContextId": live.context }), None).await.map_err(|error| live.failed(error, Step::Target))?;
        let target = created["targetId"].as_str().ok_or_else(unavailable)?.to_owned();
        let attached = live.pipe.call("Target.attachToTarget", json!({ "targetId": target, "flatten": true }), None).await.map_err(|error| live.failed(error, Step::Attach))?;
        let session = attached["sessionId"].as_str().ok_or_else(unavailable)?;
        live.profile.observe(&live.birth)?;
        live.seal_startup()?;
        let owned = json!({ "session": live.credential, "birth": daemon_birth()? });
        let origin = serde_json::to_string(&format!("http://{address}")).map_err(|_| unavailable())?;
        let source = format!("if(location.origin==={origin})Object.defineProperty(globalThis,'__cozyOwned',{{value:{owned},writable:false,configurable:false}});");
        // Blink runs new-document scripts only from an enabled Page agent; keep it enabled so reloads are injected too.
        live.pipe.call("Page.enable", json!({}), Some(session)).await.map_err(|error| live.failed(error, Step::Inject))?;
        live.pipe.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": source }), Some(session)).await.map_err(|error| live.failed(error, Step::Inject))?;
        let mut url = url::Url::parse(&format!("http://{address}/")).map_err(|_| unavailable())?;
        url.path_segments_mut().map_err(|_| unavailable())?.extend(["projects", project_ref]);
        live.pipe.call("Page.navigate", json!({ "url": url.as_str() }), Some(session)).await.map_err(|error| live.failed(error, Step::Navigate))?;
        live.profile.observe(&live.birth)?;
        live.targets.insert(project_ref.to_owned(), target);
        Ok(())
    }

    fn watch_windows(&self) {
        let (state, incomplete) = (self.live.clone(), self.retirement_incomplete.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let mut state = state.lock().await;
                let Some(live) = state.as_mut() else { return };
                let ended = live.birth.ended().unwrap_or(false);
                if !ended && live.profile.observe(&live.birth).is_err() {
                    incomplete.store(true, Ordering::Release);
                    return;
                }
                let pages_gone = if ended { true } else {
                    match live.pipe.call("Target.getTargets", json!({}), None).await {
                        Ok(targets) => targets["targetInfos"].as_array().is_some_and(|targets| !targets.iter().any(|target| target["type"] == "page" && target["browserContextId"] == live.context)),
                        Err(_) => false,
                    }
                };
                if !pages_gone { continue; }
                // Every page was explicitly closed (or the process crashed). This retires the
                // remaining owned process; it makes no claim that an interrupted edit was saved.
                incomplete.store(true, Ordering::Release);
                let Some(live) = state.take() else { return };
                let retired = Self::retire_closed(live).await;
                if retired.is_ok() { incomplete.store(false, Ordering::Release); }
                return;
            }
        });
    }

    fn processes(birth: &platform::process::Birth) -> Vec<platform::process::Exact> {
        let mut processes = if platform::process::Birth::of(birth.pid).is_ok_and(|now| &now == birth) { platform::process::tree(birth.pid) } else { Vec::new() };
        for member in platform::process::group_members(birth) {
            if processes.iter().any(|held| held.birth() == &member) { continue; }
            if let Ok(Some(exact)) = platform::process::Exact::open(&member) { processes.push(exact); }
        }
        processes
    }

    async fn retire_closed(mut live: Live) -> Result<(), Refusal> {
        let processes = Self::processes(&live.birth);
        tokio::task::spawn_blocking(move || {
            live.tree.kill().map_err(|_| unavailable())?;
            platform::process::end_tree(&processes).map_err(|_| unavailable())?;
            live.tree.wait().map_err(|_| unavailable())?;
            if platform::process::group_members(&live.birth).iter().any(|birth| !birth.ended().unwrap_or(false)) { return Err(unavailable()); }
            let proof = BrowserRetired { profile: live.profile.identity(), birth: live.birth.clone(), session: live.profile.nonce().to_owned() };
            drop(live.authority);
            live.profile.retire(proof)
        }).await.map_err(|_| unavailable())?
    }

    pub(crate) async fn prepare_close(&self) -> Result<PreparedBrowserClose, Refusal> {
        // Another admitted operation must release its lease instead of waiting behind a
        // prepared token whose owner may be waiting to close global admission.
        let mut held = self.live.clone().try_lock_owned().map_err(|_| busy())?;
        if self.retirement_incomplete.load(Ordering::Acquire) { return Err(unavailable()); }
        let mut pages = Vec::new();
        if let Some(live) = held.as_mut() {
            let targets = live.pipe.call("Target.getTargets", json!({}), None).await.map_err(|_| busy())?;
            for target in targets["targetInfos"].as_array().ok_or_else(busy)? {
                if target["type"] != "page" || target["url"] == "about:blank" { continue; }
                let url = target["url"].as_str().and_then(|url| url::Url::parse(url).ok()).ok_or_else(busy)?;
                if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") || url.port() != Some(live.address.port()) { return Err(busy()); }
                let target = target["targetId"].as_str().ok_or_else(busy)?;
                let attached = live.pipe.call("Target.attachToTarget", json!({ "targetId": target, "flatten": true }), None).await.map_err(|_| busy())?;
                pages.push(attached["sessionId"].as_str().ok_or_else(busy)?.to_owned());
            }
            let prepared = async {
                for page in &pages { Self::evaluate(live, page, "globalThis.__cozyFreeze?.() === true").await?; }
                for page in &pages { Self::evaluate(live, page, "globalThis.__cozyPrepareClose?.().then(() => true)").await?; }
                Ok::<_, Refusal>(())
            }.await;
            if prepared.is_err() {
                for page in &pages { let _ = live.pipe.call("Runtime.evaluate", json!({ "expression": "globalThis.__cozyResume?.()", "returnByValue": true }), Some(page)).await; }
                return Err(busy());
            }
        }
        Ok(PreparedBrowserClose { owner: self.live.clone(), live: Some(held), pages })
    }

    async fn evaluate(live: &mut Live, page: &str, expression: &str) -> Result<(), Refusal> {
        let result = live.pipe.call("Runtime.evaluate", json!({ "expression": expression, "awaitPromise": true, "returnByValue": true }), Some(page)).await.map_err(|_| busy())?;
        if result.get("exceptionDetails").is_some() || result["result"]["value"] != true { return Err(busy()); }
        Ok(())
    }

    pub(crate) async fn close(&self, mut prepared: PreparedBrowserClose) -> Result<(), Refusal> {
        if !Arc::ptr_eq(&self.live, &prepared.owner) { return Err(unavailable()); }
        let mut held = prepared.live.take().ok_or_else(unavailable)?;
        let Some(live) = held.take() else { return Ok(()) };
        self.retirement_incomplete.store(true, Ordering::Release);
        let processes = Self::processes(&live.birth);
        let (live, ended) = tokio::task::spawn_blocking(move || {
            let mut live = live;
            let ended = (|| {
                live.tree.kill().map_err(|_| unavailable())?;
                platform::process::end_tree(&processes).map_err(|_| unavailable())?;
                live.tree.wait().map_err(|_| unavailable())?;
                if platform::process::group_members(&live.birth).iter().any(|birth| !birth.ended().unwrap_or(false)) { return Err(unavailable()); }
                Ok::<_, Refusal>(())
            })();
            (live, ended)
        }).await.map_err(|_| unavailable())?;
        if let Err(error) = ended {
            // Ownership is retained on any uncertain retirement; neither a new session nor a
            // later lock can accidentally turn it into a successful close.
            *held = Some(live);
            return Err(error);
        }
        let proof = BrowserRetired { profile: live.profile.identity(), birth: live.birth.clone(), session: live.profile.nonce().to_owned() };
        drop(live.authority);
        live.profile.retire(proof)?;
        self.retirement_incomplete.store(false, Ordering::Release);
        Ok(())
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct BrowserOpen { project: String }

/// Native bootstrap resolves only the opaque reference. A locked project need not open its DB.
pub(crate) async fn open(State(api): State<Arc<Api>>, headers: HeaderMap, Json(ask): Json<BrowserOpen>) -> Result<Json<Value>, Refusal> {
    if !api::bearer(&headers).is_some_and(|token| api.admits(token)) {
        return Err(api::refuse(Kind::Forbidden, "browser_open_needs_bearer", "Only Cozy's native CLI can start its owned private browser"));
    }
    let reference = api.projects.reference(&api, &ask.project).map_err(Refusal)?;
    let address = *api.browser_address.get().ok_or_else(unavailable)?;
    api.browser.open(&api.sessions, address, &reference.id).await?;
    let mut url = url::Url::parse(&format!("http://{address}/")).map_err(|_| unavailable())?;
    url.path_segments_mut().map_err(|_| unavailable())?.extend(["projects", &reference.id]);
    Ok(Json(json!({ "url": url.as_str() })))
}
