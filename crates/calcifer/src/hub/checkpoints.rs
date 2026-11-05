//! What a model is, from its header: only a LoRA's factors or weights of its own, and its family. The Hub's card says neither reliably: its `family`
//! is a label, filled only from a header recorded since tfs-137. A header is small and never changes, so each is read once and only what it says is kept.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{LazyLock, Mutex},
};

use futures_util::{StreamExt as _, stream};
use hub_client::{Client, types};
use protocol::{
    config::Origin,
    hub::{FoundModel, ModelKind},
    model_ref::{ModelRef, Repo},
    tool::Tool,
};
use tensorfs::{
    Endpoint,
    family::Facts,
    format::{Header, ObjectRef},
};

use super::{quantize::distribution, reader};
use crate::api::Api;

/// How many one list reads at once.
const READS: usize = 8;
/// What's been read, by Hub and header, and by Hub and the release a package names.
static READ: LazyLock<Mutex<HashMap<String, Facts>>> = LazyLock::new(Mutex::default);

fn kept(key: &str) -> Option<Facts> {
    READ.lock().expect("read lock").get(key).cloned()
}

fn keep(key: String, facts: Facts) {
    READ.lock().expect("read lock").insert(key, facts);
}

/// Its newest release's plainest lane, else, to its members, its newest commit.
fn header_of(card: &types::ModelCard) -> Option<&types::ObjectRef> {
    let released = card.releases.first().and_then(|release| release.lanes.values().next()).map(|lane| &lane.header);
    released.or(card.commits.first().map(|commit| &commit.header))
}

/// What `header` is. One that can't be read says nothing, and a later list asks again.
async fn read(endpoint: &Endpoint, hub: &Origin, header: &types::ObjectRef) -> Option<Facts> {
    let key = format!("{hub} {}", header.sha256);
    if let Some(facts) = kept(&key) {
        return Some(facts);
    }
    let object = ObjectRef { sha256: header.sha256.clone(), length: header.length as u64 };
    let bytes = (endpoint.document(&object).await).inspect_err(|why| tracing::debug!("header {}: {}", header.sha256, why.detail)).ok()?;
    let facts = Facts::of(&Header::from_cbor(&bytes).ok()?)?;
    keep(key, facts.clone());
    Some(facts)
}

/// Each row says what it is, from the card it was read from.
pub(super) async fn said(api: &Api, hub: &Origin, cards: &[types::ModelCard], models: &mut [FoundModel]) {
    let headers: Vec<Option<types::ObjectRef>> = cards.iter().map(|card| header_of(card).cloned()).collect();
    let unread = headers.iter().flatten().any(|header| kept(&format!("{hub} {}", header.sha256)).is_none());
    let endpoint = match (unread, reader(api, hub)) {
        (true, Ok(client)) => distribution(&client, hub).await.ok(),
        _ => None,
    };
    let endpoint = endpoint.as_ref();
    let reads = headers.into_iter().map(|header| async move {
        let header = header?;
        match endpoint {
            Some(endpoint) => read(endpoint, hub, &header).await,
            None => kept(&format!("{hub} {}", header.sha256)),
        }
    });
    let facts: Vec<Option<Facts>> = stream::iter(reads).buffered(READS).collect().await;
    for (row, facts) in models.iter_mut().zip(facts) {
        let Some(facts) = facts else { continue };
        row.kind = Some(if facts.lora { ModelKind::Lora } else { ModelKind::Weights });
        row.family = facts.family.or(row.family.take());
    }
}

/// What the release a package names is.
async fn named(client: &Client, endpoint: &Endpoint, hub: &Origin, default: &str) -> Option<Facts> {
    let key = format!("{hub} {default}");
    if let Some(facts) = kept(&key) {
        return Some(facts);
    }
    let (repo, release): (Repo, Option<String>) = match ModelRef::parse(default).ok()? {
        ModelRef::Hub { repo, release } => (repo, release),
        _ => return None,
    };
    let card = (client.get_model(&repo.org, &repo.name).await).inspect_err(|why| tracing::debug!("{default}: {why}")).ok()?;
    let header = match release {
        Some(release) => card.releases.iter().find(|held| held.version == release)?.lanes.values().next().map(|lane| &lane.header),
        None => header_of(&card),
    };
    let facts = read(endpoint, hub, header?).await?;
    let label = card.model.family.clone().filter(|label| !label.is_empty());
    let facts = Facts { family: facts.family.or(label), ..facts };
    keep(key, facts.clone());
    Some(facts)
}

/// Each tool says the family of the model it runs.
pub(crate) async fn families(api: &Api, hub: &str, tools: &mut [Tool]) {
    let defaults: BTreeSet<String> = tools.iter().filter_map(|tool| tool.model.as_ref()?.default.clone()).collect();
    let Ok(origin) = Origin::parse(hub) else { return };
    let mut found: BTreeMap<String, Facts> = defaults.iter().filter_map(|default| Some((default.clone(), kept(&format!("{origin} {default}"))?))).collect();
    if found.len() < defaults.len() {
        let Ok(client) = reader(api, &origin) else { return };
        let Ok(endpoint) = distribution(&client, &origin).await else { return };
        let (client, endpoint, origin) = (&client, &endpoint, &origin);
        let unread: Vec<String> = defaults.iter().filter(|default| !found.contains_key(*default)).cloned().collect();
        let reads = unread.into_iter().map(|default| async move {
            let facts = named(client, endpoint, origin, &default).await;
            (default, facts)
        });
        let reads: Vec<(String, Option<Facts>)> = stream::iter(reads).buffered(READS).collect().await;
        found.extend(reads.into_iter().filter_map(|(default, facts)| Some((default, facts?))));
    }
    for tool in tools {
        if let Some(model) = &mut tool.model {
            model.family = model.default.as_ref().and_then(|default| found.get(default)?.family.clone());
        }
    }
}
