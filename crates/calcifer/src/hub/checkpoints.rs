//! What a model is, from its header: only a LoRA's factors or weights of its own, and its family. A model's family is the label its owner
//! gives it on the Hub, else what its header verifies as: an upload fills the label from its header, and the owner can set or clear it.
//! A header is small and never changes, so each is read once, and only what it says is kept, in a file: a first list reads every header,
//! and a restart shouldn't do that again.
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use futures_util::{StreamExt as _, stream};
use hub_client::{Client, types};
use protocol::{
    config::Origin,
    hub::{FamilyFrom, FoundModel, ModelKind},
    model_ref::{ModelRef, Repo},
    tool::Tool,
};
use serde::{Deserialize, Serialize};
use tensorfs::{
    Endpoint,
    family::{self, Facts},
    format::{Header, ObjectRef},
};

use super::{quantize::distribution, reader};
use crate::api::Api;

/// How many one list reads at once.
const READS: usize = 16;

/// What's been read, by Hub and header: true for as long as TensorFS's families are these.
#[derive(Default, Serialize, Deserialize)]
struct Read {
    families: String,
    facts: HashMap<String, Facts>,
}

static READ: OnceLock<Mutex<Read>> = OnceLock::new();

fn file(api: &Api) -> PathBuf {
    api.machines.hubs.home().calcifer().join("checkpoints.json")
}

fn read_so_far(api: &Api) -> &'static Mutex<Read> {
    READ.get_or_init(|| {
        let families = family::families_digest().to_owned();
        let held: Option<Read> = std::fs::read(file(api)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        Mutex::new(held.filter(|held| held.families == families).unwrap_or(Read { families, facts: HashMap::new() }))
    })
}

fn kept(api: &Api, key: &str) -> Option<Facts> {
    read_so_far(api).lock().expect("read lock").facts.get(key).cloned()
}

fn keep(api: &Api, key: String, facts: Facts) {
    read_so_far(api).lock().expect("read lock").facts.insert(key, facts);
}

/// What's been read goes in its file. Lost, it's read again.
fn save(api: &Api) {
    let bytes = serde_json::to_vec(&*read_so_far(api).lock().expect("read lock")).expect("what's read serializes");
    if let Err(why) = platform::durable::replace(&file(api), &bytes, 0o644) {
        tracing::debug!("{}: {why}", file(api).display());
    }
}

/// Its newest release's plainest lane, else, to its members, its newest commit.
fn header_of(card: &types::ModelCard) -> Option<&types::ObjectRef> {
    let released = card.releases.first().and_then(|release| release.lanes.values().next()).map(|lane| &lane.header);
    released.or(card.commits.first().map(|commit| &commit.header))
}

/// What `header` is. One that can't be read says nothing, and a later list asks again.
async fn read(api: &Api, endpoint: &Endpoint, hub: &Origin, header: &types::ObjectRef) -> Option<Facts> {
    let key = format!("{hub} {}", header.sha256);
    if let Some(facts) = kept(api, &key) {
        return Some(facts);
    }
    let object = ObjectRef { sha256: header.sha256.clone(), length: header.length as u64 };
    let bytes = (endpoint.document(&object).await).inspect_err(|why| tracing::debug!("header {}: {}", header.sha256, why.detail)).ok()?;
    let facts = Facts::of(&Header::from_cbor(&bytes).ok()?)?;
    keep(api, key, facts.clone());
    Some(facts)
}

/// A model's family: its owner's label, else what its weights verify as; and where it comes from. A label its weights also say is theirs.
pub(super) fn family(label: Option<String>, weights: Option<&str>) -> (Option<String>, Option<FamilyFrom>) {
    match (label.filter(|label| !label.is_empty()), weights) {
        (Some(label), Some(weights)) if label.eq_ignore_ascii_case(weights) => (Some(label), Some(FamilyFrom::Weights)),
        (Some(label), _) => (Some(label), Some(FamilyFrom::Owner)),
        (None, Some(weights)) => (Some(weights.to_owned()), Some(FamilyFrom::Weights)),
        (None, None) => (None, None),
    }
}

/// What one card's header says.
pub(super) async fn of_card(api: &Api, hub: &Origin, card: &types::ModelCard) -> Option<Facts> {
    let header = header_of(card)?;
    if let Some(facts) = kept(api, &format!("{hub} {}", header.sha256)) {
        return Some(facts);
    }
    let endpoint = distribution(&reader(api, hub).ok()?, hub).await.ok()?;
    let facts = read(api, &endpoint, hub, header).await;
    save(api);
    facts
}

/// Each row says what it is, from the card it was read from.
pub(super) async fn said(api: &Api, hub: &Origin, cards: &[types::ModelCard], models: &mut [FoundModel]) {
    let headers: Vec<Option<types::ObjectRef>> = cards.iter().map(|card| header_of(card).cloned()).collect();
    let unread = headers.iter().flatten().any(|header| kept(api, &format!("{hub} {}", header.sha256)).is_none());
    let endpoint = match (unread, reader(api, hub)) {
        (true, Ok(client)) => distribution(&client, hub).await.ok(),
        _ => None,
    };
    let endpoint = endpoint.as_ref();
    let reads = headers.into_iter().map(|header| async move {
        let header = header?;
        match endpoint {
            Some(endpoint) => read(api, endpoint, hub, &header).await,
            None => kept(api, &format!("{hub} {}", header.sha256)),
        }
    });
    let facts: Vec<Option<Facts>> = stream::iter(reads).buffered(READS).collect().await;
    if unread {
        save(api);
    }
    for (row, facts) in models.iter_mut().zip(facts) {
        let weights = facts.as_ref().and_then(|facts| facts.family.as_deref());
        (row.family, row.family_from) = family(row.family.take(), weights);
        row.kind = facts.as_ref().map(ModelKind::of);
    }
}

/// How long the family a package's model is named by stands: its owner can change the label.
const NAMED_FOR: Duration = Duration::from_secs(600);

/// The family of the model each package names, by Hub and release, when it was read.
static NAMED: OnceLock<Mutex<HashMap<String, (Instant, Option<String>)>>> = OnceLock::new();

fn named_so_far() -> &'static Mutex<HashMap<String, (Instant, Option<String>)>> {
    NAMED.get_or_init(Default::default)
}

/// The family of the release a package names: its label, else its header's.
async fn named(api: &Api, client: &Client, endpoint: &Endpoint, hub: &Origin, default: &str) -> Option<String> {
    let (repo, release): (Repo, Option<String>) = match ModelRef::parse(default).ok()? {
        ModelRef::Hub { repo, release } => (repo, release),
        _ => return None,
    };
    let card = (client.get_model(&repo.org, &repo.name).await).inspect_err(|why| tracing::debug!("{default}: {why}")).ok()?;
    let header = match release {
        Some(release) => card.releases.iter().find(|held| held.version == release)?.lanes.values().next().map(|lane| &lane.header),
        None => header_of(&card),
    };
    let weights = match header {
        Some(header) => read(api, endpoint, hub, header).await.and_then(|facts| facts.family),
        None => None,
    };
    family(card.model.family.clone(), weights.as_deref()).0
}

/// Each tool says the family of the model it runs.
pub(crate) async fn families(api: &Api, hub: &str, tools: &mut [Tool]) {
    let defaults: BTreeSet<String> = tools.iter().filter_map(|tool| tool.model.as_ref()?.default.clone()).collect();
    let Ok(origin) = Origin::parse(hub) else { return };
    let fresh = |default: &String| {
        let held = named_so_far().lock().expect("named lock");
        held.get(&format!("{origin} {default}")).filter(|(at, _)| at.elapsed() < NAMED_FOR).map(|(_, family)| family.clone())
    };
    let mut found: HashMap<String, Option<String>> = defaults.iter().filter_map(|default| Some((default.clone(), fresh(default)?))).collect();
    if found.len() < defaults.len() {
        let Ok(client) = reader(api, &origin) else { return };
        let Ok(endpoint) = distribution(&client, &origin).await else { return };
        let (client, endpoint, origin) = (&client, &endpoint, &origin);
        let unread: Vec<String> = defaults.iter().filter(|default| !found.contains_key(*default)).cloned().collect();
        let reads = unread.into_iter().map(|default| async move {
            let family = named(api, client, endpoint, origin, &default).await;
            (default, family)
        });
        let reads: Vec<(String, Option<String>)> = stream::iter(reads).buffered(READS).collect().await;
        save(api);
        let mut held = named_so_far().lock().expect("named lock");
        for (default, family) in reads {
            held.insert(format!("{origin} {default}"), (Instant::now(), family.clone()));
            found.insert(default, family);
        }
    }
    for tool in tools {
        if let Some(model) = &mut tool.model {
            model.family = model.default.as_ref().and_then(|default| found.get(default)?.clone());
        }
    }
}
