//! What an ask does once its run's machine completes it, before the run settles: an upload's
//! trigger and strength set at the Hub, an install's older releases removed. Every client gets it,
//! and one that stops following never skips it. Each is kept by the ask's idempotency key before
//! the run is submitted, so a Calcifer that stops in between does it when it next hears the
//! outcome; the result says what was done (`result.after`, `protocol::hub::AfterRun`).
use std::path::PathBuf;

use protocol::{
    calcifer::{self as wire, Environment, On, Status, SubmitRun},
    config::Origin,
    error::Error,
    hub::{AfterRun, Displayed, Retired},
    ids::RunId,
    machine::{self as v1, Source},
    model_ref::Repo,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tracing::warn;

use crate::Calcifer;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub(crate) enum After {
    /// Sets `model`'s trigger and strength: these, else (`offered`) the trigger its source gave
    /// (Civitai's trainedWords), which the machine's result names.
    Display {
        model: String,
        trigger: Option<String>,
        strength: Option<f64>,
        offered: bool,
    },
    /// Installing over an installed package updates it: the machine's older releases of `package`
    /// on the same Hub go once the new one is in. A newer one stays (a pre-release, or one the Hub
    /// yanked since).
    Retire { package: String },
}

impl Calcifer {
    fn after_path(&self, key: &str) -> PathBuf {
        self.after.join(format!("{}.json", hex::encode(Sha256::digest(key))))
    }

    /// Keeps `after` for the ask under `key`, before its run is submitted.
    pub(crate) fn plan(&self, key: &str, after: &After) -> Result<(), Error> {
        let kept = platform::durable::private_dir(&self.after).and_then(|()| {
            let bytes = serde_json::to_vec(after).expect("a step serializes");
            platform::durable::replace(&self.after_path(key), &bytes, 0o600)
        });
        kept.map_err(|e| Error::internal("calcifer_store", e.to_string()))
    }

    /// Forgets the step kept under `key`: its run was refused.
    pub(crate) fn unplan(&self, key: &str) {
        let _ = std::fs::remove_file(self.after_path(key));
    }

    /// Run `id`'s step, if its ask kept one: done when its machine succeeded, then forgotten.
    pub(crate) async fn after(&self, id: &RunId, outcome: &mut v1::Outcome) {
        let Ok(Some(key)) = self.store.key(id.as_str()) else { return };
        let path = self.after_path(&key);
        let Ok(raw) = std::fs::read(&path) else { return };
        if outcome.status == "succeeded" {
            match serde_json::from_slice::<After>(&raw) {
                Ok(after) => self.apply(id, after, outcome).await,
                Err(e) => warn!(run = %id, "an ask's step is unreadable: {e}"),
            }
        }
        if let Err(e) = std::fs::remove_file(&path) {
            warn!(run = %id, "an ask's step stays: {e}");
        }
    }

    async fn apply(&self, id: &RunId, after: After, outcome: &mut v1::Outcome) {
        let (Some(machines), Ok(Some(row))) = (self.machines(), self.store.run(id.as_str())) else { return };
        let mut result: serde_json::Value = (outcome.result.as_ref())
            .and_then(|json| serde_json::from_str(json.0.get()).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let hub = row.hub.clone().map(String::from).unwrap_or_else(|| machines.hubs.default_origin());
        let mut done = AfterRun::default();
        match after {
            After::Display { model, trigger, strength, offered } => {
                let given = result["models"][0]["trigger"].as_str().filter(|t| offered && !t.is_empty());
                let trigger = trigger.or(given.map(str::to_owned));
                if trigger.is_some() || strength.is_some() {
                    done.display = Some(display(&machines, &hub, model, trigger, strength).await);
                }
            }
            After::Retire { package } => {
                let release = result["release"].as_str().unwrap_or_default().to_owned();
                if let Some(machine) = &row.machine {
                    done.retired = retire(&machines, id, machine, &package, &release).await;
                }
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
    trigger: Option<String>,
    strength: Option<f64>,
) -> Displayed {
    let mut shown = Displayed { model, trigger, strength, error: None };
    let set = async {
        let repo = Repo::hub(&shown.model)?;
        let origin = Origin::parse(hub)?;
        let client = machines.hubs.client(&origin)?;
        let answer = client.set_model_display(&repo.org, &repo.name, shown.trigger.as_deref(), shown.strength.map(Some)).await;
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
