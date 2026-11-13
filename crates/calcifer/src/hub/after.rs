//! What an ask does once its run's machine completes it, before the run settles: an upload's
//! trigger and strength set at the Hub, an install's older releases removed. Every client gets it,
//! and one that stops following never skips it. Each is kept in `state.db` by the ask's idempotency
//! key before the run is submitted, so a Calcifer that stops in between does it when it next hears
//! the outcome; the result says what was done (`result.after`, `api::calcifer::hub::AfterRun`).

use api::{
    calcifer::{self as wire, Environment, On, Status, SubmitRun},
    config::Origin,
    error::Error,
    calcifer::hub::{AfterRun, Displayed, Retired},
    ids::RunId,
    machine::{self as v1, Source},
    model_ref::ModelId,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::Calcifer;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub(crate) enum After {
    /// Sets `model`'s triggers and strength: these, else (`offered`) the triggers its source gave
    /// (Civitai's trainedWords, each one entry), which the machine's result names.
    Display {
        model: String,
        #[serde(default)]
        triggers: Option<Vec<String>>,
        strength: Option<f64>,
        offered: bool,
    },
    /// Installing over an installed package updates it: the machine's older releases of `package`
    /// on the same Hub go once the new one is in. A newer one stays (a pre-release, or one the Hub
    /// yanked since).
    Retire { package: String },
    /// Local code installed: what its machine's Runtime read of it is kept for that source and Runtime, whatever asked for the
    /// install (a run, `cozy package install`), so what reads it next (the page's tools, `fit`) has what the machine said.
    Keep(crate::local_code::Filed),
}

impl Calcifer {
    /// Keeps `after` for the ask under `key`, before its run is submitted.
    pub(crate) fn plan(&self, key: &str, after: &After) -> Result<(), Error> {
        let step = serde_json::to_string(after).expect("a step serializes");
        self.store.plan_after(key, &step).map_err(|e| Error::internal("calcifer_store", e.to_string()))
    }

    /// Forgets the step kept under `key`: its run was refused.
    pub(crate) fn unplan(&self, key: &str) {
        if let Err(e) = self.store.forget_after(key) {
            warn!("an ask's step stays: {e}");
        }
    }

    /// Run `id`'s step, if its ask kept one: done when its machine succeeded, then forgotten.
    pub(crate) async fn after(&self, id: &RunId, outcome: &mut v1::Outcome) {
        let Ok(Some(key)) = self.store.key(id.as_str()) else { return };
        let Ok(Some(step)) = self.store.after_step(&key) else { return };
        if outcome.status == "succeeded" {
            match serde_json::from_str::<After>(&step) {
                Ok(after) => self.apply(id, after, outcome).await,
                Err(e) => warn!(run = %id, "an ask's step is unreadable: {e}"),
            }
        }
        self.unplan(&key);
    }

    async fn apply(&self, id: &RunId, after: After, outcome: &mut v1::Outcome) {
        let (Some(machines), Ok(Some(row))) = (self.machines(), self.store.run(id.as_str())) else { return };
        let mut result: serde_json::Value = (outcome.result.as_ref())
            .and_then(|json| serde_json::from_str(json.0.get()).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let hub = row.hub.clone().map(String::from).unwrap_or_else(|| machines.hubs.default_origin());
        let mut done = AfterRun::default();
        match after {
            After::Display { model, triggers, strength, offered } => {
                let given: Vec<String> = (result["models"][0]["triggers"].as_array().into_iter().flatten())
                    .filter_map(|t| t.as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned))
                    .collect();
                let triggers = triggers.or_else(|| (offered && !given.is_empty()).then_some(given));
                if triggers.is_some() || strength.is_some() {
                    done.display = Some(display(&machines, &hub, model, triggers, strength).await);
                }
            }
            After::Retire { package } => {
                let release = result["release"].as_str().unwrap_or_default().to_owned();
                if let Some(machine) = &row.machine {
                    done.retired = retire(&machines, id, machine, &package, &release).await;
                }
            }
            After::Keep(filed) => {
                let row = row.machine.as_deref().and_then(|name| machines.named(name).ok().flatten());
                let status = row.and_then(|row| machines.picture(&row).ok()?.status);
                if let Some(runtime) = status.map(|status| status.runtime).filter(|runtime| !runtime.is_empty()) {
                    crate::local_code::keep(machines.hubs.home(), &filed, &runtime, &result["interface"]);
                }
                return;
            }
        }
        if let Some(object) = result.as_object_mut() {
            object.insert("after".into(), serde_json::to_value(&done).expect("serializes"));
        }
        match v1::Json::parse(result.to_string()) {
            Ok(json) => outcome.result = Some(json),
            Err(e) => warn!(run = %id, "an ask's step is unrecordable: {e}"),
        }
    }
}

/// Sets `model`'s display metadata at `hub`; the upload stands either way.
pub(super) async fn display(
    machines: &crate::machines::Machines,
    hub: &str,
    model: String,
    triggers: Option<Vec<String>>,
    strength: Option<f64>,
) -> Displayed {
    let set_triggers = triggers.is_some();
    let mut shown = Displayed { model, triggers: triggers.unwrap_or_default(), strength, error: None };
    let set = async {
        let model_id = ModelId::hub(&shown.model)?;
        let origin = Origin::parse(hub)?;
        let client = machines.hubs.client(&origin)?;
        let triggers = set_triggers.then_some(shown.triggers.as_slice());
        let answer = client.set_model_display(&model_id.org, &model_id.name, triggers, shown.strength.map(Some)).await;
        answer.map(drop).map_err(|e| e.refusal(|| crate::rentals::login(hub)))
    };
    if let Err(e) = set.await {
        shown.error = Some(e.to_string());
    }
    shown
}

/// Removes the older releases of `package` that `machine` holds from the Hub `release` came
/// from: one remove run each, under a key named for the install, so asking again repeats none.
async fn retire(
    machines: &std::sync::Arc<crate::machines::Machines>,
    id: &RunId,
    machine: &str,
    package: &str,
    release: &str,
) -> Vec<Retired> {
    let held = match crate::machines::fresh(machines, machine).await {
        Ok(held) => held,
        Err(e) => {
            warn!(run = %id, "an install's older releases stay: {e}");
            return vec![];
        }
    };
    let envs: Vec<&Environment> = held.status.iter().flat_map(|s| &s.environments).collect();
    let canonical = |hub: &str| Origin::parse(hub).map_or_else(|_| hub.to_owned(), String::from);
    let Some(hub) = (envs.iter())
        .find(|env| env.package == package && env.release == release)
        .map(|env| canonical(&env.hub))
    else {
        return vec![];
    };
    let older = envs.iter().filter(|env| {
        env.package == package
            && canonical(&env.hub) == hub
            && env.release != release
            && tensorfs::format::newest_by([env.release.as_str(), release], |r| *r) == Some(release)
    });
    let mut retired = vec![];
    for env in older {
        let submit = SubmitRun {
            agreed: None,
            stays: vec![],
            on: On::Machine(machine.to_owned()),
            hub: hub.clone(),
            kind: wire::Kind::Remove,
            target: format!("{package}@{}", env.release),
            spec: v1::RunSpec {
                kind: v1::RunKind::Remove,
                source: Some(Source::Installation(env.installation.clone())),
                ..Default::default()
            },
            inputs: vec![],
            held: vec![],
            out: None,
            warnings: vec![],
        };
        let key = format!("retire-{id}-{}", env.installation);
        let ran = settled(&machines.calcifer, &key, submit).await;
        retired.push(Retired {
            package: package.to_owned(),
            release: env.release.clone(),
            left: ran.as_ref().is_ok_and(|run| run.status == Status::Completed),
            reason: match ran {
                Ok(run) if run.status == Status::Completed => None,
                Ok(run) => Some(failure(&run)),
                Err(e) => Some(e.to_string()),
            },
        });
    }
    retired
}

/// `submit` under `key`, waited for until it settles.
pub(crate) async fn settled(calcifer: &std::sync::Arc<Calcifer>, key: &str, submit: SubmitRun) -> Result<wire::Run, crate::Error> {
    let mut events = calcifer.subscribe();
    let run = calcifer.submit(key, submit).await?.run;
    loop {
        if let Some(row) = calcifer.store.run(run.id.as_str())?
            && row.state == crate::store::State::Settled
        {
            return calcifer.record(&row);
        }
        match events.recv().await {
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return Err(crate::Error::Invalid("Calcifer is stopping".into()));
            }
        }
    }
}

/// Why a settled run did not succeed, as `machine: reason (code)`.
pub(crate) fn failure(run: &wire::Run) -> String {
    let machine = run.machine.as_deref().unwrap_or("the machine");
    match &run.reason {
        Some(reason) => format!("{machine}: {} ({})", reason.message, reason.code),
        None => format!("{machine}: the run ended {:?}", run.status).to_lowercase(),
    }
}
