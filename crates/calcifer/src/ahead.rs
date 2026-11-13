//! What Calcifer tells each machine to have ready (cl-967, Paul 11-12): its upcoming jobs, every walk's planned jobs for it in the
//! order they would start (`PUT /v1/upcoming`); and with nothing planned for it and nothing in its queue while other machines hold
//! unstarted work it can run, that work's functions, the most first (`PUT /v1/prepare`: "if calcifer expects a large number of
//! minimax h3 jobs, and a new machine just started, telling it to download minimax h3 … so that it's ready"). Each list goes when it
//! changes, and again once its machine starts again; the machine fetches, evicts and reports. Each upcoming job's row on its board
//! moves with the downloads its machine's Status lists under it.
//!
//! On this computer's machine each project's lists go over that project's own execution link and are prepared in its own engine and
//! storage, as its runs are; the machine's own lists carry only what is no project's. Another computer's machine runs a project's
//! work as a working copy, as its runs do: one list of every project's there.
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

/// Where a list goes: a machine, and the project folder whose own engine there takes it (this computer's machine), or none: the
/// machine's own list.
type Place = (String, Option<String>);

/// Every machine's lists.
#[derive(Default)]
pub(crate) struct Lists {
    /// By machine, by project folder: what its walk planned there.
    planned: Mutex<HashMap<String, HashMap<String, Vec<Listed>>>>,
    /// By place: what it was last sent, on which boot of its machine, and the project whose link it went over.
    sent: Mutex<HashMap<Place, (Sent, Option<Arc<Project>>)>>,
}

/// Whether a project's lists on `machine` go in that project's own engine: this computer's machine's.
fn scoped(machine: &str) -> bool {
    machine.eq_ignore_ascii_case("local")
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

    /// `machine` said its Status: what its lists concern it with goes now unless this boot of it was sent it (it came up since the
    /// lists were set, or started again and holds none).
    pub(crate) fn seen(&self, machines: &Arc<Machines>, machine: &str) {
        let Some(frame) = machines.frame(machine) else { return };
        let concerned = {
            let planned = lock(&self.planned);
            planned.contains_key(machine)
                || planned.values().flat_map(|lists| lists.values().flatten()).any(|listed| listed.fits.iter().any(|fits| fits == machine))
        };
        let sent: Vec<String> = lock(&self.sent).iter().filter(|((name, _), _)| name == machine).map(|(_, (sent, _))| sent.boot.clone()).collect();
        if (!sent.is_empty() && sent.iter().all(|boot| *boot == frame.boot_id)) || (sent.is_empty() && !concerned) {
            return;
        }
        lock(&self.sent).retain(|(name, _), _| name != machine);
        self.send(machines);
    }

    /// Whether `project` (its id) has a list on `machine`: its own Status there tells its rows.
    pub(crate) fn listed_for(&self, machine: &str, project: &str) -> bool {
        lock(&self.planned).get(machine).is_some_and(|lists| lists.values().flatten().any(|listed| listed.project.scope_of().0 == project))
    }

    /// Each machine's lists as they stand, each sent where it differs from what that place was last sent.
    fn send(&self, machines: &Arc<Machines>) {
        let planned = lock(&self.planned).clone();
        // Each machine planned on, sent before, or able to run what's planned: an idle one may prepare it.
        let mut names: Vec<String> = planned.keys().cloned().collect();
        names.extend(lock(&self.sent).keys().map(|(name, _)| name.clone()));
        names.extend(planned.values().flat_map(|lists| lists.values().flatten()).flat_map(|listed| listed.fits.iter().cloned()));
        names.sort();
        names.dedup();
        for name in names {
            let Some(frame) = machines.frame(&name) else { continue };
            let mine: Vec<&Listed> = planned.get(&name).into_iter().flat_map(|lists| lists.values().flatten()).collect();
            let idle = mine.is_empty();
            // Its places: on this computer's machine each project its own, with each sent before (an emptied list goes too).
            let mut places: Vec<Option<String>> = match scoped(&name) {
                true => planned.values().flat_map(|lists| lists.iter()).filter(|(_, listed)| !listed.is_empty()).map(|(folder, _)| Some(folder.clone())).collect(),
                false => vec![None],
            };
            places.extend(lock(&self.sent).keys().filter(|(held, _)| *held == name).map(|(_, folder)| folder.clone()));
            places.sort();
            places.dedup();
            for folder in places {
                let of = |listed: &&Listed| folder.as_ref().is_none_or(|folder| listed_folder(listed) == *folder);
                let mut upcoming: Vec<&Listed> = mine.iter().copied().filter(of).collect();
                upcoming.sort_by_key(|listed| listed.start_at_ms);
                let prepare = demand(&planned, &name, &frame, idle, folder.as_deref());
                let now = Sent {
                    boot: frame.boot_id.clone(),
                    upcoming: upcoming.iter().map(|listed| listed.item.clone()).collect(),
                    prepare: prepare.iter().map(|(_, item)| item.clone()).collect(),
                };
                let place = (name.clone(), folder.clone());
                let project = match &folder {
                    Some(folder) => planned.values().flat_map(|lists| lists.get(folder)).flatten().next().map(|listed| listed.project.clone())
                        .or_else(|| lock(&self.sent).get(&place).and_then(|(_, project)| project.clone())),
                    None => None,
                };
                let was = lock(&self.sent).get(&place).map(|(sent, _)| sent.clone());
                if was.as_ref() == Some(&now) {
                    continue;
                }
                let empty = now.upcoming.is_empty() && now.prepare.is_empty();
                // A project's place with nothing there and nothing to clear, or none to send it over (gone with its project).
                if folder.is_some() && ((empty && was.is_none()) || project.is_none()) {
                    lock(&self.sent).remove(&place);
                    continue;
                }
                match empty && folder.is_some() {
                    true => drop(lock(&self.sent).remove(&place)),
                    false => drop(lock(&self.sent).insert(place.clone(), (now, project.clone()))),
                }
                let hubs: Vec<String> = upcoming.iter().map(|listed| listed.hub.clone()).chain(prepare.iter().map(|(hub, _)| hub.clone())).collect();
                let upcoming: Vec<(String, v1::Ahead)> = upcoming.iter().map(|listed| (listed.hub.clone(), listed.item.clone())).collect();
                let (machines, name) = (machines.clone(), name.clone());
                if let Some(project) = project.as_ref().filter(|_| !empty) {
                    machines.follow_project(project.scope_of().0);
                }
                machines.clone().spawn(async move {
                    if let Err(why) = put(&machines, &name, project, hubs, upcoming, prepare).await {
                        warn!(machine = name, "its upcoming jobs: {why}");
                        // Sent again at the next change.
                        lock(&machines.lists.sent).remove(&place);
                    }
                });
            }
        }
    }

    /// `machine`'s Status: each upcoming job's row there moves with its item's run and the downloads it lists under it, and is ready
    /// once that run succeeded.
    pub(crate) fn observe(&self, machine: &str, frame: &v1::StatusFrame) {
        self.observe_in(machine, None, frame);
    }

    /// `observe`, of the Status `project`'s own execution scope on `machine` pushed: that project's rows alone.
    pub(crate) fn observe_in(&self, machine: &str, project: Option<&str>, frame: &v1::StatusFrame) {
        let planned = lock(&self.planned);
        let of = |listed: &&Listed| project.is_none_or(|project| listed.project.scope_of().0 == project);
        for listed in planned.get(machine).into_iter().flat_map(|lists| lists.values().flatten()).filter(of) {
            let id = format!("{UPCOMING}{}", listed.item.key);
            let Some(run) = frame.ahead.iter().find(|run| run.id == id) else { continue };
            let ops: Vec<&v1::Operation> = frame.activity.iter().filter(|op| op.runs.contains(&id)).collect();
            let downloads: Vec<&&v1::Operation> = ops.iter().filter(|op| op.kind == "download").collect();
            let ended = matches!(run.state.as_str(), "succeeded" | "failed" | "canceled");
            let row = Row {
                ready: run.state == "succeeded",
                held_back: (run.state == "failed").then(|| format!("{machine} couldn't make it ready")),
                waiting: (!run.waiting.is_empty() && !ended).then(|| run.waiting.clone()),
                function: listed.function.clone(),
                machine: machine.to_owned(),
                run: 0,
                names: vec![listed.asset.clone()],
                stage: (!ended).then(|| match ops.first() {
                    Some(op) if op.kind == "install" => "installing it".to_owned(),
                    Some(op) => format!("downloading {}", op.subject),
                    None => "preparing it".to_owned(),
                }),
                done: downloads.iter().map(|op| op.bytes_done).sum(),
                total: downloads.iter().map(|op| op.bytes_total).sum(),
            };
            listed.project.jobs.upcoming(&id, &listed.item.key, row);
        }
    }
}

/// What `machine` prepares with no job: nothing while it has upcoming jobs or a queue; else the published functions planned on other
/// machines that it can run, the most-asked first, each once. Local code isn't: it is sent to a machine with the job that runs it there.
fn demand(planned: &HashMap<String, HashMap<String, Vec<Listed>>>, machine: &str, frame: &v1::StatusFrame, idle: bool, folder: Option<&str>) -> Vec<(String, v1::Ahead)> {
    let queued = frame.queue.as_ref().is_some_and(|queue| !queue.is_empty());
    if !idle || queued {
        return vec![];
    }
    let mut asked: Vec<(String, usize, String, v1::Ahead)> = vec![];
    for listed in (planned.iter().filter(|(name, _)| name.as_str() != machine))
        .flat_map(|(_, lists)| lists.values().flatten())
        .filter(|listed| listed.fits.iter().any(|fits| fits == machine))
        .filter(|listed| folder.is_none_or(|folder| listed_folder(listed) == folder))
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

/// A listed job's project folder, as `Lists::planned` keys it.
fn listed_folder(listed: &Listed) -> String {
    listed.project.folder_key()
}

/// Sends `machine` its lists, each item under the access its Hub gives that machine: a project's over its own execution link there,
/// as its runs go.
async fn put(machines: &Arc<Machines>, machine: &str, project: Option<Arc<Project>>, hubs: Vec<String>, upcoming: Vec<(String, v1::Ahead)>, prepare: Vec<(String, v1::Ahead)>) -> Result<(), String> {
    let (_, link) = machines.linked(machine).map_err(|e| e.message)?;
    let link = match &project {
        Some(project) => {
            let (id, root) = project.scope_of();
            crate::private_io::execution_link(id, root, machine, &link).await.map_err(|e| e.message)?
        }
        None => link,
    };
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
