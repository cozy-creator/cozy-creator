//! What Calcifer tells each machine to have ready (cl-967, Paul 11-12): its upcoming jobs, every walk's planned jobs for it in the
//! order they would start (`PUT /v1/upcoming`); and with nothing planned for it and nothing in its queue while other machines hold
//! unstarted work it can run, that work's functions, the most first (`PUT /v1/prepare`: "if calcifer expects a large number of
//! minimax h3 jobs, and a new machine just started, telling it to download minimax h3 … so that it's ready"). Each list goes when it
//! changes, and again once its machine starts again; the machine fetches, evicts and reports. Each upcoming job's row on its board
//! moves with the downloads its machine's Status lists under it.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use api::{calcifer::project::Ahead as Row, machine as v1};
use sha2::{Digest as _, Sha256};
use tracing::warn;

use crate::{machines::Machines, project::Project};

/// The prefix the machine's runs of an upcoming list take (`tensord::ahead`).
pub(crate) const UPCOMING: &str = "cozy:upcoming:";

/// One job a walk planned on a machine, as it is to be ready there.
#[derive(Clone)]
pub(crate) struct Listed {
    pub project: Arc<Project>,
    /// Its box, which its row names.
    pub asset: String,
    pub start_at_ms: i64,
    pub hub: String,
    pub item: v1::Ahead,
    /// `org/package/function`, as its row says it.
    pub function: String,
    /// The machines its plan found can run it: where it is prepared with no job.
    pub fits: Vec<String>,
}

#[derive(Clone, Default, PartialEq)]
struct Sent {
    boot: String,
    upcoming: Vec<v1::Ahead>,
    prepare: Vec<v1::Ahead>,
}

/// Every machine's lists.
#[derive(Default)]
pub(crate) struct Lists {
    /// By machine, by project folder: what its walk planned there.
    planned: Mutex<HashMap<String, HashMap<String, Vec<Listed>>>>,
    /// By machine: what it was last sent, on which boot of it.
    sent: Mutex<HashMap<String, Sent>>,
    /// The upcoming jobs whose downloads their machine has listed, by machine and key: once none is left, they're ready.
    downloaded: Mutex<std::collections::HashSet<(String, String)>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A prepare item's key: what it makes ready, the same however many jobs ask it.
fn identity(item: &v1::Ahead) -> String {
    let shape = serde_json::json!([item.source, item.entrypoint, item.models, item.lane]);
    let digest = Sha256::digest(shape.to_string().as_bytes());
    digest.iter().take(12).map(|byte| format!("{byte:02x}")).collect()
}

impl Lists {
    /// What `project`'s walk planned now, by machine: each machine's lists go again where that changed them.
    pub(crate) fn set(&self, machines: &Arc<Machines>, project: &str, by_machine: HashMap<String, Vec<Listed>>) {
        {
            let mut planned = lock(&self.planned);
            for lists in planned.values_mut() {
                lists.remove(project);
            }
            for (machine, listed) in by_machine {
                planned.entry(machine).or_default().insert(project.to_owned(), listed);
            }
            planned.retain(|_, lists| !lists.is_empty());
        }
        self.send(machines);
    }

    /// `machine` started again (its Status says another boot): it holds no list, so each goes again.
    pub(crate) fn restarted(&self, machines: &Arc<Machines>, machine: &str) {
        lock(&self.sent).remove(machine);
        self.send(machines);
    }

    /// Each machine's lists as they stand, each sent where it differs from what that machine was last sent.
    fn send(&self, machines: &Arc<Machines>) {
        let planned = lock(&self.planned).clone();
        // Each machine planned on, sent before, or able to run what's planned: an idle one may prepare it.
        let mut names: Vec<String> = planned.keys().cloned().collect();
        names.extend(lock(&self.sent).keys().cloned());
        names.extend(planned.values().flat_map(|lists| lists.values().flatten()).flat_map(|listed| listed.fits.iter().cloned()));
        names.sort();
        names.dedup();
        for name in names {
            let Some(frame) = machines.frame(&name) else { continue };
            let mut upcoming: Vec<&Listed> = planned.get(&name).into_iter().flat_map(|lists| lists.values().flatten()).collect();
            upcoming.sort_by_key(|listed| listed.start_at_ms);
            let prepare = demand(&planned, &name, &frame, upcoming.is_empty());
            let now = Sent {
                boot: frame.boot_id.clone(),
                upcoming: upcoming.iter().map(|listed| listed.item.clone()).collect(),
                prepare: prepare.iter().map(|(_, item)| item.clone()).collect(),
            };
            if lock(&self.sent).get(&name) == Some(&now) {
                continue;
            }
            lock(&self.sent).insert(name.clone(), now);
            let hubs: Vec<String> = upcoming.iter().map(|listed| listed.hub.clone()).chain(prepare.iter().map(|(hub, _)| hub.clone())).collect();
            let upcoming: Vec<(String, v1::Ahead)> = upcoming.iter().map(|listed| (listed.hub.clone(), listed.item.clone())).collect();
            let machines = machines.clone();
            machines.clone().spawn(async move {
                if let Err(why) = put(&machines, &name, hubs, upcoming, prepare).await {
                    warn!(machine = name, "its upcoming jobs: {why}");
                    // Sent again at the next change.
                    lock(&machines.lists.sent).remove(&name);
                }
            });
        }
    }

    /// `machine`'s Status: each upcoming job's row there moves with what it lists under it, and is ready once its code is installed
    /// there and what it downloaded landed.
    pub(crate) fn observe(&self, machine: &str, frame: &v1::StatusFrame) {
        let planned = lock(&self.planned);
        let mut downloaded = lock(&self.downloaded);
        for listed in planned.get(machine).into_iter().flat_map(|lists| lists.values().flatten()) {
            let id = format!("{UPCOMING}{}", listed.item.key);
            let ops: Vec<&v1::Operation> = frame.activity.iter().filter(|op| op.runs.contains(&id)).collect();
            let downloads: Vec<&&v1::Operation> = ops.iter().filter(|op| op.kind == "download").collect();
            let at = (machine.to_owned(), listed.item.key.clone());
            if !downloads.is_empty() {
                downloaded.insert(at.clone());
            }
            let ready = ops.is_empty() && downloaded.contains(&at) && installed(frame, &listed.item);
            let row = Row {
                ready,
                function: listed.function.clone(),
                machine: machine.to_owned(),
                run: 0,
                names: vec![listed.asset.clone()],
                stage: ops.first().map(|op| match op.kind.as_str() {
                    "install" => "installing it".to_owned(),
                    _ => format!("downloading {}", op.subject),
                }),
                done: downloads.iter().map(|op| op.bytes_done).sum(),
                total: downloads.iter().map(|op| op.bytes_total).sum(),
                ..Row::default()
            };
            listed.project.jobs.upcoming(&id, &listed.item.key, row);
        }
    }
}

/// Whether `frame`'s machine holds `item`'s code.
fn installed(frame: &v1::StatusFrame, item: &v1::Ahead) -> bool {
    frame.environments.iter().any(|environment| match &item.source {
        Some(v1::Source::Release(release)) => environment.package == release.package && (release.release.is_empty() || environment.release == release.release),
        Some(v1::Source::Local(local)) => environment.manifest == local.manifest,
        Some(v1::Source::Installation(alias)) => environment.installation == *alias,
        None => false,
    })
}

/// What `machine` prepares with no job: nothing while it has upcoming jobs or a queue; else the published functions planned on other
/// machines that it can run, the most-asked first, each once. Local code isn't: it is sent to a machine with the job that runs it there.
fn demand(planned: &HashMap<String, HashMap<String, Vec<Listed>>>, machine: &str, frame: &v1::StatusFrame, idle: bool) -> Vec<(String, v1::Ahead)> {
    let queued = frame.queue.as_ref().is_some_and(|queue| !queue.is_empty());
    if !idle || queued {
        return vec![];
    }
    let mut asked: Vec<(String, usize, String, v1::Ahead)> = vec![];
    for listed in (planned.iter().filter(|(name, _)| name.as_str() != machine))
        .flat_map(|(_, lists)| lists.values().flatten())
        .filter(|listed| listed.fits.iter().any(|fits| fits == machine))
        .filter(|listed| matches!(listed.item.source, Some(v1::Source::Release(_))))
    {
        let key = identity(&listed.item);
        match asked.iter_mut().find(|(held, ..)| *held == key) {
            Some((_, count, ..)) => *count += 1,
            None => asked.push((key.clone(), 1, listed.hub.clone(), v1::Ahead { key, ..listed.item.clone() })),
        }
    }
    asked.sort_by_key(|(_, count, ..)| std::cmp::Reverse(*count));
    asked.into_iter().map(|(_, _, hub, item)| (hub, item)).collect()
}

/// Sends `machine` its lists, each item under the access its Hub gives that machine.
async fn put(machines: &Arc<Machines>, machine: &str, hubs: Vec<String>, upcoming: Vec<(String, v1::Ahead)>, prepare: Vec<(String, v1::Ahead)>) -> Result<(), String> {
    let (_, link) = machines.linked(machine).map_err(|e| e.message)?;
    let mut access: HashMap<String, Option<v1::HubAccess>> = HashMap::new();
    for hub in hubs {
        if access.contains_key(&hub) {
            continue;
        }
        let specs: Vec<v1::RunSpec> = (upcoming.iter().chain(&prepare))
            .filter(|(held, _)| *held == hub)
            .map(|(_, item)| v1::RunSpec { kind: v1::RunKind::Warm, source: item.source.clone(), entrypoint: item.entrypoint.clone(), models: item.models.clone(), ..Default::default() })
            .collect();
        access.insert(hub.clone(), machines.list_access(&hub, machine, &specs).await?);
    }
    let with = |(hub, item): (String, v1::Ahead)| v1::Ahead { hub: access.get(&hub).cloned().flatten(), ..item };
    let answer = link.upcoming(&v1::Upcoming { jobs: upcoming.into_iter().map(with).collect() }).await.map_err(|e| e.to_string())?;
    for refused in answer.items.iter().filter_map(|item| item.refusal.as_ref()) {
        warn!(machine, "an upcoming job won't be ready ahead: {}", refused.message);
    }
    link.prepare(&v1::Prepare { items: prepare.into_iter().map(with).collect() }).await.map_err(|e| e.to_string())?;
    Ok(())
}
