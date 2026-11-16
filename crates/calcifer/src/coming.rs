//! A machine on its way (cl-915, Paul 11-12: "the planner … should take into account what machines are coming online"): a rental
//! booting or ordered, quoted as its machine will quote once it serves. It should serve when its Hub's word says
//! (`Hubs::on_its_way`); then it is quoted as a machine that holds nothing (`placement::Picture`, `api::estimate::quote`), the runs
//! already sent to it ahead of the jobs asked, each timed by this computer's runs on its card, else its release's benchmarks (cl-1024:
//! of that card, else another's scaled by the Hub's speeds), else not at all (a minute, as a machine assumes).
use std::sync::Arc;

use api::{
    calcifer::{Kind, MachineKind, MachineState, On, Status},
    machine::{self as v1, Basis, JobSize},
};

use crate::{
    api::Api,
    machines::Machines,
    placement::Picture,
    rentals::OnItsWay,
    store::RunFilter,
};

/// How many of the runs already sent to it are counted.
const QUEUED: u64 = 32;

/// A rental of this controller with no address yet.
pub(crate) struct Coming {
    pub name: String,
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
        Some(Coming { name: row.name, way })
    }
}

/// `coming` as it will picture itself once it serves (`placement::Picture`): its Hub's GPUs, nothing held, nothing timed; the runs
/// already sent to it, which wait for it here, ahead of anything asked, each timed by its card (`card_time`).
pub(crate) async fn picture(api: &Arc<Api>, coming: &Coming) -> Picture {
    let way = &coming.way;
    let gpus = (0..way.gpus).map(|index| v1::Gpu { index, name: way.card.clone(), ..Default::default() }).collect();
    let frame = v1::StatusFrame { gpus, serving_gpus: way.gpus, ..Default::default() };
    let filter = RunFilter { status: Some(Status::Queued), machine: Some(coming.name.clone()), limit: QUEUED, ..RunFilter::default() };
    let sent = api.calcifer.each_store(|store| store.runs(&filter).map(|(rows, ..)| rows)).map(|lists| lists.into_iter().flatten().collect::<Vec<_>>()).unwrap_or_default();
    let mut ahead = vec![];
    for row in sent.iter().filter(|row| row.kind == Kind::Call) {
        let payload: serde_json::Value = row.spec.payload.as_ref().and_then(|p| serde_json::from_str(p.get()).ok()).unwrap_or_default();
        let size = JobSize::of(&payload);
        let (one_ms, _) = card_time(api, way, &row.target, &size);
        ahead.push(v1::EstimateJob { key: row.id.to_string(), function: row.target.clone(), size, gpus: row.spec.gpus, prior_ms: one_ms as u64, ..Default::default() });
    }
    Picture { on: On::Machine(coming.name.clone()), name: coming.name.clone(), frame, ready_at_ms: Some(way.ready_at_ms), ahead, references: Default::default() }
}

/// What a run of `target` (`org/pkg/fn@release`, `local/pkg/fn`) of `size` takes on one GPU of `way`'s card, and what that stands on:
/// this computer's runs on that card; else nothing (0), where its release's benchmarks time it, as the estimator reads them for the
/// card (cl-1024), else a minute.
pub(crate) fn card_time(api: &Arc<Api>, way: &OnItsWay, target: &str, size: &JobSize) -> (f64, Basis) {
    match api.calcifer.card_ms(target, &way.card, size) {
        Some(ms) => (ms, Basis::Prior),
        None => (0.0, Basis::None),
    }
}
