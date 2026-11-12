//! A machine on its way (cl-915, Paul 11-12: "the planner … should take into account what machines are coming online"): a rental
//! booting or ordered, quoted as its machine will quote once it serves. It should serve when its Hub's word says
//! (`Hubs::on_its_way`); then each job prepares as on a machine that holds nothing (`estimate::prepare_ms`), and the runs already sent
//! to it and the jobs asked are packed on its GPUs as its machine packs them (`estimate::pack`), each timed by this computer's runs on
//! its card, else its Hub's runs of that card, else not at all (a minute, as a machine assumes).
use std::sync::Arc;

use api::{
    calcifer::{Kind, MachineKind, MachineState, On, Status},
    estimate::{self, INSTALL_MS, LINK, Scaling, UNKNOWN_MS},
    machine::{self as v1, Basis, JobSize},
};

use crate::{
    api::Api,
    eta::same_card,
    machines::Machines,
    placement::{Job, Offer},
    rentals::OnItsWay,
    store::RunFilter,
};

/// How many of the runs already sent to it are counted.
const QUEUED: u64 = 32;
/// Within this share of a run's size, it is that size.
const SAME_SIZE: f64 = 0.15;

/// A rental of this controller with no address yet.
pub(crate) struct Coming {
    pub name: String,
    pub hub: String,
    pub way: OnItsWay,
}

impl Machines {
    /// `name` when it is a rental on its way: no address yet, not ending, and coming by a time known. None for any other machine.
    pub(crate) fn coming(&self, name: &str, now: i64) -> Option<Coming> {
        let row = self.store().machine(name).ok()??;
        if row.kind != MachineKind::Rental || row.endpoint.is_some() || row.state != MachineState::Booting {
            return None;
        }
        if self.pending_end(&row.name).ok()?.is_some() {
            return None;
        }
        let way = self.hubs.on_its_way(&row, now)?;
        Some(Coming { name: row.name, hub: row.hub, way })
    }
}

/// When `coming` would run each of `jobs` were they all sent now, after the runs already sent to it, which wait for it here.
pub(crate) async fn quotes(api: &Arc<Api>, coming: &Coming, jobs: &[&Job], now: i64) -> Vec<Offer> {
    let way = &coming.way;
    let gpus = way.gpus as usize;
    let ready_in = (way.ready_at_ms - now).max(0) as f64;
    let mut queue = vec![];
    let filter = RunFilter { status: Some(Status::Queued), machine: Some(coming.name.clone()), limit: QUEUED, ..RunFilter::default() };
    let sent = api.calcifer.store().runs(&filter).map(|(rows, ..)| rows).unwrap_or_default();
    for row in sent.iter().filter(|row| row.kind == Kind::Call) {
        let payload: serde_json::Value = row.spec.payload.as_ref().and_then(|p| serde_json::from_str(p.get()).ok()).unwrap_or_default();
        let (one_ms, _) = card_time(api, &coming.hub, way, &row.target, &JobSize::of(&payload)).await;
        let widths = estimate::unheld_widths(gpus, row.spec.gpus);
        queue.push(estimate::Job { widths, one_ms, scaling: Scaling::PRIOR, ready_ms: ready_in + INSTALL_MS, running: None });
    }
    let mut asked = vec![];
    for job in jobs {
        let (one_ms, basis) = card_time(api, &coming.hub, way, &job.target, &job.size).await;
        let prepare_ms = estimate::prepare_ms(false, job.models.iter().map(|model| model.bytes).sum(), LINK, 0.0);
        let mut offer = Offer {
            machine: On::Machine(coming.name.clone()),
            fits: true,
            refusal: None,
            width: 0,
            start_at_ms: 0,
            finish_at_ms: 0,
            prepare_ms: prepare_ms as u64,
            queue_position: 0,
            basis,
            ready_at_ms: Some(way.ready_at_ms),
        };
        if job.gpu && gpus == 0 {
            offer.fits = false;
            offer.refusal = Some(v1::Reason {
                code: "no_gpu".into(),
                message: format!("{} has no GPU, and it builds a model on one", coming.name),
                origin: "machine".into(),
                ..Default::default()
            });
            asked.push((None, offer));
            continue;
        }
        let widths = estimate::unheld_widths(gpus, job.gpus);
        queue.push(estimate::Job { widths, one_ms, scaling: Scaling::PRIOR, ready_ms: ready_in + prepare_ms, running: None });
        asked.push((Some(queue.len() - 1), offer));
    }
    let placed = estimate::pack(gpus, &queue);
    (asked.into_iter())
        .map(|(at, mut offer)| {
            if let Some(place) = at.map(|at| placed[at]) {
                offer.width = place.width;
                offer.start_at_ms = now + place.start_ms as i64;
                offer.finish_at_ms = now + place.finish_ms as i64;
                offer.queue_position = place.position as u32;
            }
            offer
        })
        .collect()
}

/// What a run of `target` (`org/pkg/fn@release`, `local/pkg/fn`) of `size` takes on one GPU of `way`'s card, and what that stands on:
/// this computer's runs on that card, else `hub`'s of it (a published function's), else nothing (a minute).
pub(crate) async fn card_time(api: &Arc<Api>, hub: &str, way: &OnItsWay, target: &str, size: &JobSize) -> (f64, Basis) {
    if let Some(ms) = api.calcifer.card_ms(target, &way.card, size) {
        return (ms, Basis::Prior);
    }
    match hub_ms(api, hub, way, target, size).await {
        Some(ms) => (ms, Basis::Prior),
        None => (UNKNOWN_MS, Basis::None),
    }
}

/// What `hub` says `target` took on `way`'s product, on one GPU at `size`: its runs of that size, else all of them scaled to it.
async fn hub_ms(api: &Arc<Api>, hub: &str, way: &OnItsWay, target: &str, size: &JobSize) -> Option<f64> {
    let function = target.split('@').next()?;
    if function.starts_with("local/") || function.matches('/').count() != 2 {
        return None;
    }
    let said = api.machines.hubs.run_times(hub, function).await?;
    let ours = |sku: &str| (!way.sku.is_empty() && sku.eq_ignore_ascii_case(&way.sku)) || same_card(sku, &way.card);
    let runs: Vec<_> = said.skus.iter().filter(|times| ours(&times.sku)).flat_map(|times| &times.runs).collect();
    let one = |run: &&::api::hub::types::RunTime| run.execution_ms as f64 / Scaling::PRIOR.at(run.width);
    let same: Vec<f64> = runs.iter().filter(|run| (size.ratio(&run.size) - 1.0).abs() <= SAME_SIZE).map(one).collect();
    let like = match same.is_empty() {
        false => same,
        true => runs.iter().map(|run| one(run) * size.ratio(&run.size)).collect(),
    };
    median(like)
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    (n > 0).then(|| if n % 2 == 1 { values[n / 2] } else { (values[n / 2 - 1] + values[n / 2]) / 2.0 })
}
