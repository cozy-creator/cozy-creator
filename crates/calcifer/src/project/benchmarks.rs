//! `GET /v1/projects/{id}/benchmarks?package=`: a package's benchmarks from this project's runs of it (cl-1024), what `cozy package
//! benchmarks` writes to `benchmarks.json` for its next release to carry. Each completed warm run of one of its functions on a GPU,
//! grouped by function, GPU, count and request: each group's median warm time, and its cold runs' median. Nothing is run or measured
//! here: only what its runs measured.
use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::{Path, Query, State, rejection::QueryRejection},
};
use api::{
    calcifer::{Status, project::{Benchmarks, BenchmarksAsk}},
    estimate::{Benchmark, BenchmarkRanOn},
    machine::JobSize,
};
use serde_json::Value;

use crate::{api::{Api, Refusal}, store::RunFilter};

/// The newest runs of a project read for them.
const READ: u64 = 2000;

pub(super) async fn benchmarks(State(api): State<Arc<Api>>, Path(id): Path<String>, query: Result<Query<BenchmarksAsk>, QueryRejection>) -> Result<Json<Benchmarks>, Refusal> {
    let Query(asked) = query?;
    let project = api.projects.get(&api, &id)?;
    let name = asked.package.rsplit('/').next().unwrap_or(&asked.package).to_owned();
    let filter = RunFilter { status: Some(Status::Completed), limit: READ, ..Default::default() };
    let (rows, ..) = api.calcifer.project_store(&project.id)?.runs(&filter)?;
    let named = project.folder.dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    // By function, GPU, count and request: each run's time and whether it was warm, oldest first.
    let mut groups: BTreeMap<(String, String, u32, String), (JobSize, Vec<(u64, bool)>)> = BTreeMap::new();
    for row in rows.iter().rev().filter(|row| row.kind == api::calcifer::Kind::Call) {
        let base = row.target.split('@').next().unwrap_or_default();
        let mut parts = base.rsplit('/');
        let (Some(function), Some(package)) = (parts.next(), parts.next()) else { continue };
        let Some(outcome) = row.outcome.as_ref().filter(|outcome| !outcome.gpu.is_empty() && outcome.execution_ms > 0 && !outcome.memoized) else { continue };
        if package != name {
            continue;
        }
        let measured: Value = outcome.measurements.as_ref().and_then(|m| serde_json::from_str(m.get()).ok()).unwrap_or_default();
        let gpus = measured["execution"]["degree"].as_u64().and_then(|d| u32::try_from(d).ok()).unwrap_or(1).max(1);
        let payload: Value = row.spec.payload.as_ref().and_then(|p| serde_json::from_str(p.get()).ok()).unwrap_or_default();
        // Its size as its run's record keeps it (its function's own variables, cl-1286), else the guess from its request.
        let request = serde_json::from_value(measured["size"].clone()).ok().filter(|size: &JobSize| !size.values.is_empty()).unwrap_or_else(|| JobSize::of(&payload));
        let key = (function.to_owned(), outcome.gpu.clone(), gpus, serde_json::to_string(&request).unwrap_or_default());
        let warm = crate::reports::warm(row, &measured).unwrap_or(false);
        groups.entry(key).or_insert_with(|| (request, vec![])).1.push((outcome.execution_ms, warm));
    }
    let cozy = crate::update::this_release().to_owned();
    let benchmarks = (groups.into_iter())
        .filter_map(|((function, gpu, gpus, _), (request, runs))| {
            let warm: Vec<u64> = runs.iter().filter(|(_, warm)| *warm).map(|(ms, _)| *ms).collect();
            let cold: Vec<u64> = runs.iter().filter(|(_, warm)| !*warm).map(|(ms, _)| *ms).collect();
            Some(Benchmark {
                function,
                gpu,
                gpus,
                request,
                warm_ms: median(&warm)?,
                cold_ms: median(&cold),
                ran_on: Some(BenchmarkRanOn { cozy: cozy.clone(), runtime: String::new() }),
                note: format!("{} warm runs in project {named}", warm.len()),
                ..Benchmark::default()
            })
        })
        .collect();
    Ok(Json(Benchmarks { benchmarks }))
}

fn median(values: &[u64]) -> Option<u64> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    (n > 0).then(|| if n % 2 == 1 { sorted[n / 2] } else { (sorted[n / 2 - 1] + sorted[n / 2]) / 2 })
}
