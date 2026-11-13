//! What this rental measured of its own host, told to the Hub once per pod per signal (th-364),
//! which ranks a host under its floor after every healthy one: TensorFS's transfer from the origin
//! once 1 GiB has arrived, over the time any of it was arriving, and the first spawned executor's
//! cold imports of the Runtime and torch. Each is one call, never retried; a Hub without the route
//! answers 404 and that is fine. A marker in the machine's state keeps a restarted TensorD from
//! reporting again. A loaded run's heat (cl-913: how much of it a GPU was held back for heat) is told
//! at the first such run and whenever it turns hot or cool; a Hub from before it refuses the field,
//! which is no harm.
use super::hub::Hub;
use ::api::hub::types::{WorkerDownload, WorkerGpuHeat, WorkerProbe};
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};

/// Under this a transfer measures its latency more than its link (the Hub judges none smaller).
const MEASURED_BYTES: u64 = 1 << 30;

struct Reporter {
    hub: Arc<Hub>,
    state: PathBuf,
    runtime: tokio::runtime::Handle,
}

static REPORTER: OnceLock<Reporter> = OnceLock::new();

/// Reports for this rental from now on, on the current runtime; `origin` is its one downloader.
pub(crate) fn serve(hub: Arc<Hub>, state: PathBuf, origin: Arc<crate::origin::Machine>) {
    let runtime = tokio::runtime::Handle::current();
    if REPORTER
        .set(Reporter {
            hub,
            state,
            runtime: runtime.clone(),
        })
        .is_err()
        || reported("download")
    {
        return;
    }
    let returned = origin.returned();
    runtime.spawn(async move {
        loop {
            returned.notified().await;
            let (bytes, time) = origin.received();
            if bytes >= MEASURED_BYTES && !time.is_zero() {
                let download = WorkerDownload {
                    bytes,
                    seconds: time.as_secs_f64(),
                };
                return send("download", Some(download), None, None);
            }
        }
    });
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
        match hub.report_speed(None, None, Some(heat)).await {
            Ok(()) => tracing::info!("reported this host's GPU heat to the Hub: {what}"),
            Err(refusal) => tracing::info!("the Hub took no GPU heat report: {refusal}"),
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
        match hub.report_speed(download, cpu, heat).await {
            Ok(()) => tracing::info!("reported this host's {signal} speed to the Hub: {what}"),
            Err(refusal) => tracing::info!("the Hub took no {signal} speed report: {refusal}"),
        }
    });
}
