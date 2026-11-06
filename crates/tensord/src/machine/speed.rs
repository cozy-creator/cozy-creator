//! What this rental measured of its own host, told to the Hub once per pod per signal (th-364),
//! which ranks a host under its floor after every healthy one: TensorFS's transfer from the origin
//! once 1 GiB has arrived, over the time any of it was arriving, and the first spawned executor's
//! cold imports of the Runtime and torch. Each is one call, never retried; a Hub without the route
//! answers 404 and that is fine. A marker in the machine's state keeps a restarted TensorD from
//! reporting again.
use super::hub::Hub;
use hub_client::types::{WorkerDownload, WorkerProbe};
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};

/// Under this a transfer measures its latency more than its link (the Hub judges none smaller).
const MEASURED_BYTES: u64 = 1 << 30;
const LOOK: Duration = Duration::from_secs(5);

struct Reporter {
    hub: Arc<Hub>,
    state: PathBuf,
    runtime: tokio::runtime::Handle,
}

static REPORTER: OnceLock<Reporter> = OnceLock::new();

/// Reports for this rental from now on, on the current runtime; `origin` is its one downloader.
pub fn serve(hub: Arc<Hub>, state: PathBuf, origin: Arc<crate::origin::Machine>) {
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
    runtime.spawn(async move {
        loop {
            tokio::time::sleep(LOOK).await;
            let (bytes, time) = origin.received();
            if bytes >= MEASURED_BYTES && !time.is_zero() {
                let download = WorkerDownload {
                    bytes,
                    seconds: time.as_secs_f64(),
                };
                return send("download", Some(download), None);
            }
        }
    });
}

/// The first executor this pod spawned (an import-only parent, or a session no parent forks):
/// its launch to Hello, which imports the interpreter's Runtime and with it torch, plus its Start's
/// `torch_import` leg where torch is imported there. The same work on every host of an image,
/// before any package is imported.
pub fn imported(hello: Duration, stages: &[(String, f64)]) {
    if REPORTER.get().is_none() || reported("cpu") {
        return;
    }
    let leg = stages.iter().find(|(leg, _)| leg == "torch_import").map_or(0.0, |(_, ms)| ms / 1e3);
    let seconds = hello.as_secs_f64() + leg;
    send("cpu", None, Some(WorkerProbe { probe: "executor_import".into(), seconds }));
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
fn send(signal: &'static str, download: Option<WorkerDownload>, cpu: Option<WorkerProbe>) {
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
        match hub.report_speed(download, cpu).await {
            Ok(()) => tracing::info!("reported this host's {signal} speed to the Hub: {what}"),
            Err(refusal) => tracing::info!("the Hub took no {signal} speed report: {refusal}"),
        }
    });
}
