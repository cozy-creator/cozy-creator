//! What this rental measured of its own host, told to the Hub once per pod per signal (th-364),
//! which ranks a host under its floor after every healthy one: its first download's pace where the link set it
//! (`first_download`), and the first spawned executor's cold imports of the Runtime and torch. Each is one call,
//! never retried; a Hub without the route answers 404 and that is fine. A marker in the machine's state keeps a
//! restarted TensorD from reporting again. A loaded run's heat (cl-913: how much of it a GPU was held back for heat) is told
//! at the first such run and whenever it turns hot or cool; a Hub from before it refuses the field,
//! which is no harm. Its GPU power limit against the card's default (cl-1161) is told the same way, in a
//! report of its own, so a Hub that refuses it still takes the heat.
use super::hub::Hub;
use ::api::hub::types::{WorkerDownload, WorkerGpuHeat, WorkerGpuPower, WorkerLinkFloor, WorkerProbe};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tensorfs::fetch::Sustained;

/// Under this a transfer measures its latency more than its link (the Hub judges none smaller).
const MEASURED_BYTES: u64 = 1 << 30;
/// The pace held to the floor is the link's over its last 10 s.
const TRAILING: Duration = Duration::from_secs(10);
/// A transfer whose pace the link set none of for this long is over: the next is measured afresh, its own warm-up apart.
const GAP: Duration = Duration::from_secs(5);
/// A transfer's first seconds, left out, and how long it is measured before it is told, where no lease says (an older Hub).
const WARMUP: Duration = Duration::from_secs(10);
const WINDOW: Duration = Duration::from_secs(30);

struct Reporter {
    hub: Arc<Hub>,
    state: PathBuf,
    runtime: tokio::runtime::Handle,
}

static REPORTER: OnceLock<Reporter> = OnceLock::new();

/// The floor this rental's lease names for its first download (th-405); none from a Hub before it.
static FLOOR: Mutex<Option<WorkerLinkFloor>> = Mutex::new(None);

/// The lease as last read (`lifecycle::keep_authority`).
pub(crate) fn floor(leased: Option<WorkerLinkFloor>) {
    *FLOOR.lock().unwrap_or_else(|e| e.into_inner()) = leased.filter(|floor| floor.mbps > 0 && floor.window_seconds > 0);
}

/// Reports for this rental from now on, on the current runtime.
pub(crate) fn serve(hub: Arc<Hub>, state: PathBuf) {
    let runtime = tokio::runtime::Handle::current();
    if REPORTER
        .set(Reporter {
            hub,
            state,
            runtime: runtime.clone(),
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
    /// The counts since, a window each, back to the newest at least `TRAILING` old.
    trail: VecDeque<Sustained>,
    /// The count since which the pace of the last `TRAILING` has stayed under the floor.
    under: Option<Sustained>,
}

/// What a transfer says of its host's link, once.
enum Said {
    /// Under the lease's floor through its whole window (th-405): these bytes over these seconds.
    Slow(u64, Duration, WorkerLinkFloor),
    /// What it sustained past its warm-up.
    Measured(u64, Duration),
}

fn mbps(bytes: u64, time: Duration) -> f64 {
    bytes as f64 * 8.0 / 1e6 / time.as_secs_f64().max(1e-3)
}

impl Transfer {
    /// The link's count went from `before` to `now` in one more window. Under `floor` for its whole window, the transfer is
    /// slow. Otherwise, measured that long with 1 GiB landed and its pace not now under the floor, it is told as measured.
    fn window(&mut self, before: Sustained, now: Sustained, floor: Option<WorkerLinkFloor>) -> Option<Said> {
        let seconds = |s: u32| Duration::from_secs(s.into());
        let (warmup, window) = floor.map_or((WARMUP, WINDOW), |floor| (seconds(floor.warmup_seconds), seconds(floor.window_seconds)));
        let Some(warm) = self.warm else {
            if now.time - self.began.time >= warmup {
                self.warm = Some(now);
                self.trail.push_back(now);
            }
            return None;
        };
        self.trail.push_back(now);
        while self.trail.len() > 2 && now.time - self.trail[1].time >= TRAILING {
            self.trail.pop_front();
        }
        let trailing = mbps(now.bytes - self.trail[0].bytes, now.time - self.trail[0].time);
        match floor.filter(|floor| trailing < f64::from(floor.mbps)) {
            Some(_) => _ = self.under.get_or_insert(before),
            None => self.under = None,
        }
        if let (Some(under), Some(floor)) = (self.under, floor) {
            let (bytes, time) = (now.bytes - under.bytes, now.time - under.time);
            if time < window {
                return None;
            }
            // Every 10 s of it under the floor, so is the whole, but for a window's rounding: said only when it is.
            if mbps(bytes, time) < f64::from(floor.mbps) {
                return Some(Said::Slow(bytes, time, floor));
            }
            self.under = None;
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

/// Holds this rental's downloads to its lease's floor until one says what its host's link is (th-405, Paul 11-18, after a host
/// pulled 29 GiB at 14.7 MB/s: "make sure we don't run on slow hosts like that again please"). Each window TensorFS says the
/// link set the pace in is one step; nothing runs between downloads. A transfer under the floor for the lease's window is
/// reported as a slow link, and the Hub answers whether it is placing the rental on another host. Else its sustained pace is
/// reported as the host's download speed: once measured for the window with 1 GiB landed, or when it ends with 1 GiB.
async fn first_download() {
    let mut link = tensorfs::fetch::sustained();
    let mut seen = *link.borrow_and_update();
    let mut transfer: Option<Transfer> = None;
    loop {
        let moved = match transfer {
            Some(_) => tokio::time::timeout(GAP, link.changed()).await.ok(),
            None => Some(link.changed().await),
        };
        let said = match moved {
            Some(Err(_)) => return,
            None => transfer.take().and_then(|ended| ended.over(seen)),
            Some(Ok(())) => {
                let (before, now) = (seen, *link.borrow_and_update());
                seen = now;
                let floor = *FLOOR.lock().unwrap_or_else(|e| e.into_inner());
                let began = || Transfer { began: before, warm: None, trail: VecDeque::new(), under: None };
                transfer.get_or_insert_with(began).window(before, now, floor)
            }
        };
        match said {
            Some(Said::Slow(bytes, time, floor)) => return slow(bytes, time, floor).await,
            Some(Said::Measured(bytes, time)) => {
                return send("download", Some(WorkerDownload { bytes, seconds: time.as_secs_f64() }), None, None);
            }
            None => {}
        }
    }
}

/// Tells the Hub this host's link is under its floor, once per pod, and says its answer.
async fn slow(bytes: u64, time: Duration, floor: WorkerLinkFloor) {
    let (Some(reporter), Some(path)) = (REPORTER.get(), marker("slow-link")) else { return };
    if std::fs::OpenOptions::new().write(true).create_new(true).open(&path).is_err() {
        return;
    }
    let measured = format!(
        "this host's link carried {:.0} Mbit/s over {:.0} s of its first download, under the Hub's floor of {} Mbit/s",
        mbps(bytes, time),
        time.as_secs_f64(),
        floor.mbps
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
