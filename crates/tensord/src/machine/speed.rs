//! What this rental measured of its own host, told to the Hub once per pod per signal (th-364),
//! which ranks a host under its floor after every healthy one: its first download's pace where the link set it
//! (`first_download`), and the first spawned executor's cold imports of the Runtime and torch. Each is one call,
//! never retried; a Hub without the route answers 404 and that is fine. A marker in the machine's state keeps a
//! restarted TensorD from reporting again. A loaded run's heat (cl-913: how much of it a GPU was held back for heat) is told
//! at the first such run and whenever it turns hot or cool; a Hub from before it refuses the field,
//! which is no harm. Its GPU power limit against the card's default (cl-1161) is told the same way, in a
//! report of its own, so a Hub that refuses it still takes the heat.
use super::hub::Hub;
use ::api::hub::types::{WorkerDownload, WorkerGpuHeat, WorkerGpuPower, WorkerProbe, WorkerSlowLinkRule};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tensorfs::{fetch::Sustained, store::Store};

/// Under this a transfer measures its latency more than its link (the Hub judges none smaller).
const MEASURED_BYTES: u64 = 1 << 30;
/// A transfer whose pace the link set none of for this long is over: the next is measured afresh, its own warm-up apart.
const GAP: Duration = Duration::from_secs(5);
/// A transfer's first seconds, left out, and how long it is measured before its download is told, where no lease names a
/// rule (a Hub before th-405).
const WARMUP: Duration = Duration::from_secs(10);
const WINDOW: Duration = Duration::from_secs(30);

struct Reporter {
    hub: Arc<Hub>,
    state: PathBuf,
    runtime: tokio::runtime::Handle,
    /// What the machine holds on its disk (`SlowLinkRule::landed_bytes_max`).
    store: Arc<Store>,
}

static REPORTER: OnceLock<Reporter> = OnceLock::new();

/// The slow-link rule this rental's lease names (th-405); none from a Hub before it.
static RULE: Mutex<Option<WorkerSlowLinkRule>> = Mutex::new(None);
/// When this machine first read its lease: a new machine is judged for `within_seconds` from here.
static LEASED: OnceLock<Instant> = OnceLock::new();

/// The lease as last read (`lifecycle::keep_authority`).
pub(crate) fn rule(leased: Option<WorkerSlowLinkRule>) {
    LEASED.get_or_init(Instant::now);
    let leased = leased.filter(|rule| rule.mbps > 0 && rule.window_seconds > 0 && rule.recover_seconds > 0);
    *RULE.lock().unwrap_or_else(|e| e.into_inner()) = leased;
}

/// The lease's rule, and whether this machine is still new by it.
fn leased() -> (Option<WorkerSlowLinkRule>, bool) {
    let rule = *RULE.lock().unwrap_or_else(|e| e.into_inner());
    let new = rule.zip(LEASED.get()).is_some_and(|(rule, at)| at.elapsed() < Duration::from_secs(rule.within_seconds.into()));
    (rule, new)
}

/// Reports for this rental from now on, on the current runtime.
pub(crate) fn serve(hub: Arc<Hub>, state: PathBuf, store: Arc<Store>) {
    let runtime = tokio::runtime::Handle::current();
    if REPORTER
        .set(Reporter {
            hub,
            state,
            runtime: runtime.clone(),
            store,
        })
        .is_err()
        || reported("download")
        || reported("slow-link")
    {
        return;
    }
    runtime.spawn(first_download());
}

/// One transfer as the link paced it (`tensorfs::fetch::Sustained`: the windows the link, not what was asked of it, set the
/// pace): from the link's count when it began, its warm-up apart.
struct Transfer {
    began: Sustained,
    /// The count when its warm-up ended; None while it warms up.
    warm: Option<Sustained>,
    /// The counts since, a window each, back to the newest at least the rule's `recover_seconds` old.
    trail: VecDeque<Sustained>,
    /// The count since which the pace of the last `recover_seconds` has stayed under the rule.
    under: Option<Sustained>,
    /// Held to the lease's rule: the machine's first transfer, while the machine is new.
    judged: bool,
    /// What its disk held when it began (`SlowLinkRule::landed_bytes_max`); judged only.
    held: u64,
}

/// What a transfer says of its host's link, once.
enum Said {
    /// Under the lease's rule through its whole window (th-405): these bytes over these seconds.
    Slow(u64, Duration, WorkerSlowLinkRule),
    /// What it sustained past its warm-up.
    Measured(u64, Duration),
}

fn mbps(bytes: u64, time: Duration) -> f64 {
    bytes as f64 * 8.0 / 1e6 / time.as_secs_f64().max(1e-3)
}

impl Transfer {
    /// The link's count went from `before` to `now` in one more window. Judged, under `rule` for its `window_seconds` with no
    /// `recover_seconds` back at it, the transfer is slow. Otherwise, measured that long with 1 GiB landed and its pace not
    /// now under the rule, it is told as measured.
    fn window(&mut self, before: Sustained, now: Sustained, rule: Option<WorkerSlowLinkRule>) -> Option<Said> {
        let seconds = |s: u32| Duration::from_secs(s.into());
        let (warmup, window) = rule.map_or((WARMUP, WINDOW), |rule| (seconds(rule.warmup_seconds), seconds(rule.window_seconds)));
        let Some(warm) = self.warm else {
            if now.time - self.began.time >= warmup {
                self.warm = Some(now);
                self.trail.push_back(now);
            }
            return None;
        };
        match rule.filter(|_| self.judged) {
            None => self.under = None,
            Some(rule) => {
                self.trail.push_back(now);
                while self.trail.len() > 2 && now.time - self.trail[1].time >= seconds(rule.recover_seconds) {
                    self.trail.pop_front();
                }
                let trailing = mbps(now.bytes - self.trail[0].bytes, now.time - self.trail[0].time);
                if trailing < f64::from(rule.mbps) {
                    self.under.get_or_insert(before);
                } else {
                    self.under = None;
                }
                if let Some(under) = self.under {
                    let (bytes, time) = (now.bytes - under.bytes, now.time - under.time);
                    if time < window {
                        return None;
                    }
                    // Every trailing stretch of it under the rule, so is the whole, but for a window's rounding: said only when it is.
                    if mbps(bytes, time) < f64::from(rule.mbps) {
                        return Some(Said::Slow(bytes, time, rule));
                    }
                    self.under = None;
                }
            }
        }
        let (bytes, time) = (now.bytes - warm.bytes, now.time - warm.time);
        (time >= window && bytes >= MEASURED_BYTES).then_some(Said::Measured(bytes, time))
    }

    /// It is over at count `now`: what it sustained past its warm-up, if that is enough to judge a link by.
    fn over(&self, now: Sustained) -> Option<Said> {
        let warm = self.warm?;
        (now.bytes - warm.bytes >= MEASURED_BYTES).then(|| Said::Measured(now.bytes - warm.bytes, now.time - warm.time))
    }
}

/// Measures this rental's downloads until one says what its host's link is, and holds a new machine's first one to its lease's
/// slow-link rule (th-405). Paul 11-18: "maybe end it if it's way below that for a good while and not coming back, especially if
/// it's a new machine that we just got … never end a machine with lots of stuff on its hard-drive already because its download
/// link takes a crap later on." Each window TensorFS says the link set the pace in is one step; nothing runs between downloads.
/// The machine's first transfer, while it is within the rule's `within_seconds` of its lease, is judged: under the rule for its
/// window with no recovery, and with under `landed_bytes_max` on its disk (what it held when the transfer began, and what the
/// transfer landed since), it is reported as a slow link, and the Hub answers
/// whether it is placing the rental on another host. Every other is the host's download speed, its sustained pace reported
/// once: measured for the window with 1 GiB landed, or when it ends with 1 GiB.
async fn first_download() {
    let mut link = tensorfs::fetch::sustained();
    let mut seen = *link.borrow_and_update();
    let mut transfer: Option<Transfer> = None;
    // Only the machine's first transfer is judged.
    let mut first = true;
    loop {
        let moved = match transfer {
            Some(_) => tokio::time::timeout(GAP, link.changed()).await.ok(),
            None => Some(link.changed().await),
        };
        let said = match moved {
            Some(Err(_)) => return,
            None => {
                first = false;
                transfer.take().and_then(|ended| ended.over(seen))
            }
            Some(Ok(())) => {
                let (before, now) = (seen, *link.borrow_and_update());
                let (rule, new) = leased();
                if transfer.is_none() {
                    let judged = first && new && rule.is_some();
                    let held = if judged { held().await } else { 0 };
                    transfer = Some(Transfer { began: before, warm: None, trail: VecDeque::new(), under: None, judged, held });
                }
                seen = now;
                let current = transfer.as_mut().expect("begun above");
                current.judged &= new;
                current.window(before, now, rule)
            }
        };
        match said {
            Some(Said::Slow(bytes, time, rule)) => {
                // What it held when this began and what this landed since.
                let landed = transfer.as_ref().map_or(u64::MAX, |current| current.held.saturating_add(seen.bytes - current.began.bytes));
                if landed < rule.landed_bytes_max {
                    return slow(bytes, time, rule).await;
                }
                tracing::info!(
                    "this host's link carried {:.0} Mbit/s over {:.0} s of its first download, under the Hub's {} Mbit/s, but the \
                     machine holds {:.1} GB: not reported slow",
                    mbps(bytes, time),
                    time.as_secs_f64(),
                    rule.mbps,
                    landed as f64 / 1e9
                );
                if let Some(current) = transfer.as_mut() {
                    current.judged = false;
                }
            }
            Some(Said::Measured(bytes, time)) => {
                return send("download", Some(WorkerDownload { bytes, seconds: time.as_secs_f64() }), None, None);
            }
            None => {}
        }
    }
}

/// The bytes on this machine's disk; unreadable, as many as can be (it is then never ended for its link).
async fn held() -> u64 {
    let Some(store) = REPORTER.get().map(|reporter| reporter.store.clone()) else { return u64::MAX };
    match tokio::task::spawn_blocking(move || store.landed()).await {
        Ok(Ok(landed)) => landed,
        Ok(Err(refusal)) => {
            tracing::info!("what this machine holds is unreadable: {refusal}");
            u64::MAX
        }
        Err(_) => u64::MAX,
    }
}

/// Tells the Hub this host's link is under its rule, once per pod, and says its answer.
async fn slow(bytes: u64, time: Duration, rule: WorkerSlowLinkRule) {
    let (Some(reporter), Some(path)) = (REPORTER.get(), marker("slow-link")) else { return };
    if std::fs::OpenOptions::new().write(true).create_new(true).open(&path).is_err() {
        return;
    }
    let measured = format!(
        "this host's link carried {:.0} Mbit/s over {:.0} s of its first download, under the Hub's {} Mbit/s",
        mbps(bytes, time),
        time.as_secs_f64(),
        rule.mbps
    );
    match reporter.hub.report_slow_link(bytes, time.as_secs_f64()).await {
        Ok(true) => tracing::warn!("{measured}: the Hub is placing this rental on another host"),
        Ok(false) => tracing::warn!("{measured}: the Hub keeps this rental here"),
        Err(refusal) => tracing::warn!("{measured}; the Hub took no word of it: {refusal}"),
    }
}

/// The first executor this pod spawned (an import-only parent, or a session no parent forks):
/// its launch to Hello, which imports the interpreter's Runtime and with it torch, plus its Start's
/// `torch_import` leg where torch is imported there. The same work on every host of an image,
/// before any package is imported.
pub(crate) fn imported(hello: Duration, stages: &[(String, f64)]) {
    if REPORTER.get().is_none() || reported("cpu") {
        return;
    }
    let leg = stages.iter().find(|(leg, _)| leg == "torch_import").map_or(0.0, |(_, ms)| ms / 1e3);
    let seconds = hello.as_secs_f64() + leg;
    send("cpu", None, Some(WorkerProbe { probe: "executor_import".into(), seconds }), None);
}

/// A loaded run on this pod ended with a GPU held back for heat `share` of its `seconds` busy, the most of its GPUs.
pub(crate) fn heat(share: f32, seconds: f64) {
    static TOLD: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);
    let Some(reporter) = REPORTER.get() else { return };
    let hot = share > platform::gpu::HOT_SHARE;
    if TOLD.lock().unwrap().replace(hot) == Some(hot) {
        return;
    }
    let hub = reporter.hub.clone();
    let heat = WorkerGpuHeat { share, seconds };
    reporter.runtime.spawn(async move {
        let what = format!("{heat:?}");
        match hub.report_speed(None, None, Some(heat), None).await {
            Ok(()) => tracing::info!("reported this host's GPU heat to the Hub: {what}"),
            Err(refusal) => tracing::info!("the Hub took no GPU heat report: {refusal}"),
        }
    });
}

/// A loaded run on this pod ended on GPUs whose driver enforces `limit_w` of the card's `default_w`, the lowest of its GPUs against
/// its default, over `seconds` busy: told at the first such run and whenever the limits change.
pub(crate) fn power(limit_w: u32, default_w: u32, seconds: f64) {
    static TOLD: std::sync::Mutex<Option<(u32, u32)>> = std::sync::Mutex::new(None);
    let Some(reporter) = REPORTER.get() else { return };
    if TOLD.lock().unwrap().replace((limit_w, default_w)) == Some((limit_w, default_w)) {
        return;
    }
    let hub = reporter.hub.clone();
    let power = WorkerGpuPower { power_cap_watts: limit_w, power_default_watts: default_w, seconds };
    reporter.runtime.spawn(async move {
        let what = format!("{power:?}");
        match hub.report_speed(None, None, None, Some(power)).await {
            Ok(()) => tracing::info!("reported this host's GPU power limit to the Hub: {what}"),
            Err(refusal) => tracing::info!("the Hub took no GPU power report: {refusal}"),
        }
    });
}

fn marker(signal: &str) -> Option<PathBuf> {
    REPORTER
        .get()
        .map(|reporter| reporter.state.join(format!("speed-{signal}.reported")))
}

fn reported(signal: &str) -> bool {
    marker(signal).is_none_or(|path| path.exists())
}

/// One call for `signal`: only the caller that creates its marker sends it.
fn send(signal: &'static str, download: Option<WorkerDownload>, cpu: Option<WorkerProbe>, heat: Option<WorkerGpuHeat>) {
    let (Some(reporter), Some(path)) = (REPORTER.get(), marker(signal)) else {
        return;
    };
    if std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .is_err()
    {
        return;
    }
    let hub = reporter.hub.clone();
    reporter.runtime.spawn(async move {
        let what = format!("{download:?} {cpu:?}");
        match hub.report_speed(download, cpu, heat, None).await {
            Ok(()) => tracing::info!("reported this host's {signal} speed to the Hub: {what}"),
            Err(refusal) => tracing::info!("the Hub took no {signal} speed report: {refusal}"),
        }
    });
}
