//! What a model is, from its header: only a LoRA's factors or weights of its own, and its family. A model's family is the label its owner
//! gives it on the Hub, else what its header verifies as: an upload fills the label from its header, and the owner can set or clear it.
//! The Hub's catalog carries only that label, and not for a model uploaded before it was filled: what its weights verify as, and whether
//! it's a LoRA, are read here until the Hub derives them at publish (tensorhub th-367). A header is small and never changes, so each is
//! read once, and only what it says is kept, in `state.db`'s `headers`: a first list reads every header, and a restart shouldn't.
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use futures_util::{StreamExt as _, stream};
use ::api::hub::{Client, types};
use api::{
    config::Origin,
    calcifer::hub::{FamilyFrom, FoundModel, ModelContentKind},
    model_ref::{ModelRef, ModelId},
    calcifer::tool::Tool,
};
use tensorfs::{
    Endpoint,
    family::{self, Facts},
    format::{Header, ObjectRef},
};

use super::{quantize::distribution, reader};
use crate::api::Api;

/// How many one list reads at once.
const READS: usize = 16;

/// A header, by its Hub and sha256.
type Key = (String, String);

/// What's been read while TensorFS's families are these, and what of it isn't in `state.db` yet.
struct Read {
    facts: HashMap<Key, Facts>,
    unkept: Vec<(Key, Facts)>,
}

static READ: OnceLock<Mutex<Read>> = OnceLock::new();

fn read_so_far(api: &Api) -> &'static Mutex<Read> {
    READ.get_or_init(|| {
        let facts = api.calcifer.store().header_facts(family::families_digest()).unwrap_or_else(|why| {
            tracing::warn!("{why}: model headers are read again");
            HashMap::new()
        });
        Mutex::new(Read { facts, unkept: vec![] })
    })
}

fn key(hub: &Origin, header: &types::ObjectRef) -> Key {
    (hub.to_string(), header.sha256.clone())
}

fn kept(api: &Api, key: &Key) -> Option<Facts> {
    read_so_far(api).lock().expect("read lock").facts.get(key).cloned()
}

fn keep(api: &Api, key: Key, facts: Facts) {
    let mut read = read_so_far(api).lock().expect("read lock");
    read.facts.insert(key.clone(), facts.clone());
    read.unkept.push((key, facts));
}

/// What's been read since goes in `state.db`, in one transaction. Lost, it's read again.
fn save(api: &Api) {
    let unkept = std::mem::take(&mut read_so_far(api).lock().expect("read lock").unkept);
    if unkept.is_empty() {
        return;
    }
    if let Err(why) = api.calcifer.store().keep_header_facts(family::families_digest(), &unkept) {
        tracing::debug!("keeping what headers said: {why}");
    }
}

/// Its newest release's plainest lane, else, to its members, its newest checkpoint.
fn header_of(card: &types::ModelCard) -> Option<&types::ObjectRef> {
    let released = card.releases.iter().find(|release| release.visibility == api::model_ref::ReleaseVisibility::Public).and_then(|release| release.lanes.values().next()).map(|lane| &lane.header);
    released.or(card.checkpoints.first().map(|checkpoint| &checkpoint.header))
}

/// What `header` is. One that can't be read says nothing, and a later list asks again.
async fn read(api: &Api, endpoint: &Endpoint, hub: &Origin, header: &types::ObjectRef) -> Option<Facts> {
    let key = key(hub, header);
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
    if let Some(facts) = kept(api, &key(hub, header)) {
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
    let unread = headers.iter().flatten().any(|header| kept(api, &key(hub, header)).is_none());
    let endpoint = match (unread, reader(api, hub)) {
        (true, Ok(client)) => distribution(&client, hub).await.ok(),
        _ => None,
    };
    let endpoint = endpoint.as_ref();
    let reads = headers.into_iter().map(|header| async move {
        let header = header?;
        match endpoint {
            Some(endpoint) => read(api, endpoint, hub, &header).await,
            None => kept(api, &key(hub, &header)),
        }
    });
    let facts: Vec<Option<Facts>> = stream::iter(reads).buffered(READS).collect().await;
    if unread {
        save(api);
    }
    for (row, facts) in models.iter_mut().zip(facts) {
        let weights = facts.as_ref().and_then(|facts| facts.family.as_deref());
        (row.family, row.family_from) = family(row.family.take(), weights);
        row.content_kind = facts.as_ref().map(content_kind);
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
    let (model_id, release): (ModelId, Option<String>) = match ModelRef::parse(default).ok()? {
        ModelRef::Hub { model: model_id, release } => (model_id, release),
        _ => return None,
    };
    let card = (client.get_model(&model_id.org, &model_id.name).await).inspect_err(|why| tracing::debug!("{default}: {why}")).ok()?;
    // The Hub's label is its family: only a model with none is read for what its header says.
    if let Some(label) = card.model.family.clone().filter(|label| !label.is_empty()) {
        return Some(label);
    }
    let header = match release {
        Some(release) => card.releases.iter().find(|held| held.version == release)?.lanes.values().next().map(|lane| &lane.header),
        None => header_of(&card),
    };
    read(api, endpoint, hub, header?).await?.family
}

/// Each declared slot says the family of its default model.
pub(crate) async fn families(api: &Api, hub: &str, tools: &mut [Tool]) {
    let defaults: BTreeSet<String> = tools.iter().flat_map(|tool| tool.models.iter().chain(tool.model.iter())).filter_map(|model| model.default.clone()).collect();
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
        for model in tool.models.iter_mut().chain(tool.model.iter_mut()) {
            model.family = model.default.as_ref().and_then(|default| found.get(default)?.clone());
        }
    }
}

/// What a header's facts say a checkpoint is: a LoRA, else a family's model, else a component.
pub(crate) fn content_kind(facts: &Facts) -> ModelContentKind {
    match (facts.lora, &facts.family) {
        (true, _) => ModelContentKind::Lora,
        (false, Some(_)) => ModelContentKind::Weights,
        (false, None) => ModelContentKind::Component,
    }
}
