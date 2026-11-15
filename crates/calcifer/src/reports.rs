//! Anonymous performance data (cl-1006, Paul 11-15): each published function's run on any machine, local, added or rented, goes to
//! the Hub as its function, package, versions, GPU, size, times, kernels and outcome. Never its inputs, outputs, names or keys, and
//! sent with no credential over connections of its own, so nothing ties it to who ran it. Off (`calcifer.send_performance_data =
//! false`, the page's Settings, `cozy config set`): nothing is queued or sent. Sending is off every run's path: reports wait in
//! memory and go together, 64 at once or 10 minutes after the first, and a batch the Hub doesn't take is dropped.
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use api::{
    calcifer::{Kind, Settings},
    config,
    error::{Error, Kind as ErrorKind},
    machine::{self as v1, JobSize},
};
use ::api::hub::{
    Client, Credential,
    types::{ReportedGpu, ReportedModel, ReportedSize, ReportedSoftware, ReportedTiming, RunReport, RunReports},
};
use serde_json::Value;

use crate::{
    eta::Took,
    machines::{Machines, lock},
    store::RunRow,
};

/// The most reports one request carries, as the Hub takes them.
const BATCH: usize = 64;
/// The most waiting: past it, the oldest is dropped.
const HELD: usize = 256;
/// From a batch's first report to its sending.
const WAIT: Duration = Duration::from_secs(600);

/// Whether reports are sent, and those waiting.
pub(crate) struct Reports {
    send: AtomicBool,
    hub: config::Origin,
    /// Its own connections: none a signed-in call used.
    http: reqwest::Client,
    wait: Duration,
    waiting: Mutex<Waiting>,
}

#[derive(Default)]
struct Waiting {
    runs: Vec<RunReport>,
    /// A sending is due.
    timed: bool,
}

impl Reports {
    pub(crate) fn new(config: &config::Config) -> Self {
        Self {
            send: AtomicBool::new(config.calcifer.send_performance_data.unwrap_or(true)),
            hub: config.hub_url.clone(),
            http: ::api::hub::http(None).expect("the webpki roots alone"),
            wait: config.calcifer.report_seconds.map_or(WAIT, |s| Duration::from_secs(s.into())),
            waiting: Mutex::default(),
        }
    }

    pub(crate) fn settings(&self) -> Settings {
        Settings { send_performance_data: self.send.load(Ordering::Relaxed) }
    }
}

impl Machines {
    /// Changes this computer's settings: written to config.toml first, so a Calcifer started next keeps them. Turned off, what
    /// waits is dropped.
    pub(crate) fn change_settings(&self, settings: &Settings) -> Result<Settings, Error> {
        config::set_send_performance_data(&self.home, settings.send_performance_data)?;
        self.reports.send.store(settings.send_performance_data, Ordering::Relaxed);
        if !settings.send_performance_data {
            *lock(&self.reports.waiting) = Waiting::default();
        }
        Ok(self.reports.settings())
    }

    /// Queues a settled run's report, unless performance data is off or the run is not one to report.
    pub(crate) fn report_run(self: &Arc<Self>, row: &RunRow, outcome: &v1::Outcome) {
        if !self.reports.send.load(Ordering::Relaxed) {
            return;
        }
        let frame = row.machine.as_deref().and_then(|machine| self.frame(machine));
        let Some(run) = report(row, outcome, frame.as_ref()) else { return };
        let due = {
            let mut waiting = lock(&self.reports.waiting);
            if waiting.runs.len() >= HELD {
                waiting.runs.remove(0);
            }
            waiting.runs.push(run);
            let due = match (waiting.runs.len() >= BATCH, waiting.timed) {
                (true, _) => Some(Duration::ZERO),
                (false, false) => Some(self.reports.wait),
                (false, true) => None,
            };
            waiting.timed |= due.is_some();
            due
        };
        if let Some(wait) = due {
            let me = self.clone();
            self.spawn(async move {
                tokio::time::sleep(wait).await;
                me.send_reports().await;
            });
        }
    }

    async fn send_reports(&self) {
        let runs = {
            let mut waiting = lock(&self.reports.waiting);
            waiting.timed = false;
            std::mem::take(&mut waiting.runs)
        };
        if runs.is_empty() || !self.reports.send.load(Ordering::Relaxed) {
            return;
        }
        let client = Client::connect(&self.reports.hub, self.reports.http.clone(), Credential::Anonymous);
        for batch in runs.chunks(BATCH) {
            if let Err(e) = client.report_runs(&RunReports { runs: batch.to_vec() }).await {
                tracing::debug!(runs = runs.len(), "run reports dropped: {e}");
                return;
            }
        }
    }
}

/// A settled run as its report says it: a published function's call that ran on its machine. None for any other.
fn report(row: &RunRow, outcome: &v1::Outcome, frame: Option<&v1::StatusFrame>) -> Option<RunReport> {
    let Some(v1::Source::Release(release)) = &row.spec.source else { return None };
    if row.kind != Kind::Call || outcome.memoized || outcome.execution_ms == 0 || !reference(&release.package) {
        return None;
    }
    let outcome_name = match outcome.status.as_str() {
        status @ ("succeeded" | "failed" | "canceled") => status.to_owned(),
        _ => return None,
    };
    let version = match release.release.as_str() {
        "" => row.target.rsplit_once('@').map(|(_, at)| at.to_owned()).unwrap_or_default(),
        named => named.to_owned(),
    };
    if version.is_empty() || !version.chars().all(|c| c.is_ascii_alphanumeric() || ".+_-".contains(c)) {
        return None;
    }
    let measured: Value = outcome.measurements.as_ref().and_then(|m| serde_json::from_str(m.get()).ok()).unwrap_or_default();
    let payload: Value = row.spec.payload.as_ref().and_then(|p| serde_json::from_str(p.get()).ok()).unwrap_or_default();
    let job = JobSize::of(&payload);
    let pixels = |key: &str| payload.get(key).and_then(Value::as_u64).filter(|n| *n > 0);
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let software = &measured["software"];
    let construction = &measured["execution"]["construction"];
    let prepared = construction["prepared_unix_ms"].as_f64().filter(|at| *at > 0.0);
    let warm = prepared.zip(row.started_at_ms).map(|(at, started)| at <= started as f64);
    let steps = row.started_at_ms.and_then(|started| {
        let took = Took::of("", "", started, outcome.execution_ms, outcome.measurements.as_ref()?.get(), None)?;
        let main = took.stages.iter().filter_map(|stage| stage.steps.map(|steps| (stage.ms, steps))).max_by(|a, b| a.0.total_cmp(&b.0));
        main.map(|(_, steps)| steps)
    });
    let peak = measured["memory"]["driver"].as_array().into_iter().flatten().filter_map(|gpu| gpu["peak_bytes"].as_u64()).max();
    let gpu = frame.and_then(|frame| {
        let first = frame.gpus.first()?;
        let name = if outcome.gpu.is_empty() { first.name.clone() } else { outcome.gpu.clone() };
        Some(ReportedGpu {
            name,
            count: u32::try_from(frame.gpus.len()).unwrap_or(u32::MAX),
            memory_gib: (first.memory_bytes > 0).then(|| (first.memory_bytes as f64 / GIB).round() as u32),
            driver: first.driver.clone(),
            family: first.family.clone(),
        })
    });
    let mut kernels: Vec<String> = measured["observations"].as_array().into_iter().flatten()
        .filter_map(|row| row["name"].as_str())
        .filter(|name| kernel(name))
        .map(str::to_owned)
        .collect();
    kernels.sort();
    kernels.dedup();
    kernels.truncate(64);
    Some(RunReport {
        function: format!("{}/{}", release.package, row.spec.entrypoint),
        release: version,
        models: measured["models"].as_array().into_iter().flatten().filter_map(model).take(16).collect(),
        software: ReportedSoftware {
            cozy: ::api::VERSION.to_owned(),
            tensord: frame.map(|frame| frame.version.clone()).filter(|v| !v.is_empty()).unwrap_or_else(|| text(&software["tensord"])),
            runtime: text(&software["runtime"]),
            tensorfs: text(&software["tensorfs"]),
        },
        os: frame.map(|frame| frame.platform.clone()).unwrap_or_default(),
        width: measured["execution"]["degree"].as_u64().and_then(|d| u32::try_from(d).ok()).unwrap_or(1).max(1),
        gpu,
        size: ReportedSize {
            megapixels: job.megapixels,
            width: pixels("width"),
            height: pixels("height"),
            frames: job.frames,
            steps: job.steps,
            tokens: job.tokens,
        },
        timing: ReportedTiming {
            total_ms: outcome.execution_ms,
            load_ms: construction["prepare_ms"].as_f64().filter(|_| warm == Some(false)).map(|ms| ms.round() as u64),
            steps: steps.map(|s| s.count),
            step1_ms: steps.map(|s| s.first_ms),
            steady_ms: steps.map(|s| s.steady_ms),
            warm,
            peak_vram_gib: peak.map(|bytes| (bytes as f64 / GIB * 10.0).round() / 10.0),
        },
        kernels,
        outcome: outcome_name,
        error_code: outcome.reason.as_ref().map(|reason| reason.code.clone()).filter(|code| self::code(code)).unwrap_or_default(),
    })
}

const GIB: f64 = (1u64 << 30) as f64;

/// A model slot as its run's measurements name it, when it names a Hub model: never a path or a digest.
fn model(row: &Value) -> Option<ReportedModel> {
    let text = |key: &str| row[key].as_str().unwrap_or_default().to_owned();
    let model = text("repository");
    if !reference(&model) {
        return None;
    }
    let adapters = row["adapters"].as_array().into_iter().flatten()
        .filter_map(|adapter| adapter["model"].as_str())
        .filter(|name| reference(name))
        .map(str::to_owned)
        .take(16)
        .collect();
    let word = |value: String| value.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)).then_some(value).unwrap_or_default();
    Some(ReportedModel {
        slot: word(text("slot")),
        model,
        release: word(text("release")),
        lane: word(text("lane")),
        precision: word(text("precision")),
        adapters,
    })
}

/// `org/name`, or `org/name@release`: a Hub reference, never a path.
fn reference(name: &str) -> bool {
    let (repository, release) = name.split_once('@').unwrap_or((name, ""));
    let word = |part: &str| !part.is_empty() && part.len() <= 128 && part.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    let mut parts = repository.split('/');
    let shape = matches!((parts.next(), parts.next(), parts.next()), (Some(org), Some(repo), None) if word(org) && word(repo) && !org.starts_with('.'));
    shape && (release.is_empty() || word(release)) && !repository.ends_with(".safetensors")
}

fn kernel(name: &str) -> bool {
    let Some((family, rest)) = name.split_once('.') else { return false };
    ["attention", "gemm", "kernel", "request_path"].contains(&family)
        && (1..=96).contains(&rest.len())
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c))
}

fn code(code: &str) -> bool {
    (1..=64).contains(&code.len()) && code.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.".contains(c))
}

/// `GET /v1/settings`.
pub(crate) async fn show(axum::extract::State(api): axum::extract::State<Arc<crate::api::Api>>) -> axum::Json<Settings> {
    axum::Json(api.machines.reports.settings())
}

/// `PUT /v1/settings`: the whole of them.
pub(crate) async fn change(
    axum::extract::State(api): axum::extract::State<Arc<crate::api::Api>>,
    axum::Json(settings): axum::Json<Settings>,
) -> Result<axum::Json<Settings>, crate::api::Refusal> {
    let machines = api.machines.clone();
    let changed = tokio::task::spawn_blocking(move || machines.change_settings(&settings))
        .await
        .map_err(|e| crate::api::Refusal(Error::new(ErrorKind::Internal, "settings_write", e.to_string())))?;
    changed.map(axum::Json).map_err(crate::api::Refusal)
}
