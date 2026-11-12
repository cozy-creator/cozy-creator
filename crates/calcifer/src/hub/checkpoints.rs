//! What a model is, from its header: only a LoRA's factors or weights of its own, its family, and what it holds as a model slot reads it.
//! A model's family is the label its owner gives it on the Hub, else what its header verifies as; it groups and finds models and never
//! decides what loads where: a slot loads what holds what its default holds (cl-943, [`tensorfs::family::parts`]). The Hub's catalog
//! carries only the label, so headers are read here. A header is small and never changes, so each is read once, and only what it says is
//! kept, in `state.db`'s `headers`: a first list reads every header, and a restart shouldn't.
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use futures_util::{StreamExt as _, stream};
use ::api::hub::{Client, types};
use api::{
    config::Origin,
    choices::Choice,
    error::{Error, Kind},
    calcifer::hub::{FamilyFrom, FoundModel, ModelContentKind},
    interface::{Callable, Interface, Slot},
    model_ref::{ModelKind, ModelRef},
    calcifer::tool::Tool,
};
use tensorfs::{
    Endpoint,
    family::{self, Facts, parts::Reads},
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
        let facts = api.calcifer.store().header_facts(family::facts_digest()).unwrap_or_else(|why| {
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
    if let Err(why) = api.calcifer.store().keep_header_facts(family::facts_digest(), &unkept) {
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

/// Each row says what it is, from the card it was read from; and what each header said.
pub(super) async fn said(api: &Api, hub: &Origin, cards: &[types::ModelCard], models: &mut [FoundModel]) -> Vec<Option<Facts>> {
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
    for (row, facts) in models.iter_mut().zip(&facts) {
        let weights = facts.as_ref().and_then(|facts| facts.family.as_deref());
        (row.family, row.family_from) = family(row.family.take(), weights);
        row.content_kind = facts.as_ref().map(content_kind);
    }
    facts
}

/// How long the family a package's model is named by stands: its owner can change the label.
const NAMED_FOR: Duration = Duration::from_secs(600);

/// The family and kind of each declared default, by Hub and reference, when read.
#[derive(Clone, Default)]
struct DefaultModel {
    family: Option<String>,
    kind: Option<ModelKind>,
}

static NAMED: OnceLock<Mutex<HashMap<String, (Instant, DefaultModel)>>> = OnceLock::new();

fn named_so_far() -> &'static Mutex<HashMap<String, (Instant, DefaultModel)>> {
    NAMED.get_or_init(Default::default)
}

/// A declared default: its family label or header, and its real checkpoint kind.
async fn named(api: &Api, client: &Client, endpoint: &Endpoint, hub: &Origin, default: &str) -> Option<DefaultModel> {
    let reference = ModelRef::parse(default).ok()?;
    let model_id = match &reference {
        ModelRef::Hub { model, .. } | ModelRef::Checkpoint { model, .. } => model,
        _ => return None,
    };
    let card = (client.get_model(&model_id.org, &model_id.name).await).inspect_err(|why| tracing::debug!("{default}: {why}")).ok()?;
    let facts = match reference_header(&card, &reference) {
        Some(header) => read(api, endpoint, hub, header).await,
        None => None,
    };
    Some(DefaultModel {
        family: family(card.model.family.clone(), facts.as_ref().and_then(|facts| facts.family.as_deref())).0,
        kind: known_kind(&card, facts.as_ref()),
    })
}

fn known_kind(card: &types::ModelCard, facts: Option<&Facts>) -> Option<ModelKind> {
    facts.map(|facts| if facts.lora { ModelKind::Lora } else { ModelKind::Base })
        .or(card.model.kind).or_else(|| card.model.role.as_deref().and_then(ModelKind::from_role))
}

fn reference_header<'a>(card: &'a types::ModelCard, reference: &ModelRef) -> Option<&'a types::ObjectRef> {
    match reference {
        ModelRef::Hub { release: Some(release), .. } => card.releases.iter().find(|held| held.version == *release)?.lanes.values().next().map(|lane| &lane.header),
        ModelRef::Hub { release: None, .. } => header_of(card),
        ModelRef::Checkpoint { checkpoint, .. } => card.checkpoints.iter().find(|held| held.id == checkpoint.as_str()).map(|held| &held.header)
            .or_else(|| card.releases.iter().flat_map(|release| release.lanes.values()).find(|lane| lane.checkpoint == checkpoint.as_str()).map(|lane| &lane.header)),
        _ => None,
    }
}

/// The release a named one resolves to: the newest public one of its series after it, as TensorD resolves it (xs-183).
fn newest_fix<'a>(release: &'a str, card: &'a types::ModelCard) -> &'a str {
    let public = card.releases.iter().filter(|held| held.visibility == api::model_ref::ReleaseVisibility::Public).map(|held| held.version.as_str());
    api::model_ref::newest_fix(release, public).unwrap_or(release)
}

/// What `reference` is, as the run that names it loads it: a named release as the newest fix it resolves to, an unnamed one as the
/// newest, a checkpoint as itself. None: it isn't a Hub model, or its card or header can't be read now.
async fn held(api: &Api, client: &Client, endpoint: Option<&Endpoint>, hub: &Origin, reference: &str) -> Option<(types::ModelCard, Facts)> {
    let parsed = ModelRef::parse(reference).ok()?;
    let model = match &parsed {
        ModelRef::Hub { model, .. } | ModelRef::Checkpoint { model, .. } => model,
        _ => return None,
    };
    let card = client.get_model(&model.org, &model.name).await.inspect_err(|why| tracing::debug!("{reference}: {why}")).ok()?;
    let parsed = match parsed {
        ModelRef::Hub { model, release: Some(release) } => ModelRef::Hub { release: Some(newest_fix(&release, &card).to_owned()), model },
        other => other,
    };
    let header = reference_header(&card, &parsed)?;
    let facts = match endpoint {
        Some(endpoint) => read(api, endpoint, hub, header).await,
        None => kept(api, &key(hub, header)),
    }?;
    Some((card, facts))
}

/// The family a slot's LoRAs are judged by: what the header says, else the owner's label.
fn adapts(card: &types::ModelCard, facts: &Facts) -> Option<String> {
    facts.family.clone().or_else(|| card.model.family.clone().filter(|label| !label.is_empty()))
}

/// A function's slots as its choices name them: its own by parameter, its calls' by path.
pub(crate) fn slots_of<'a>(interface: &'a Interface, callable: &'a Callable) -> Vec<(&'a str, &'a Slot)> {
    let own = callable.models.iter().map(|slot| (slot.param.as_str(), slot));
    own.chain(interface.call_slots(callable).into_iter().map(|slot| (slot.path.as_str(), slot))).collect()
}

/// The slots `key` names, as the binder takes it (`Choices::slots`): its own by parameter, a call's by path, else every call's of
/// that parameter.
pub(crate) fn slots_named<'a>(interface: &'a Interface, callable: &'a Callable, key: &str) -> Vec<&'a Slot> {
    let all = slots_of(interface, callable);
    let exact: Vec<&Slot> = all.iter().filter(|(held, _)| *held == key).map(|(_, slot)| *slot).collect();
    match exact.is_empty() {
        true => all.into_iter().filter(|(_, slot)| slot.param == key).map(|(_, slot)| slot).collect(),
        false => exact,
    }
}

/// How `slot` reads its checkpoint: the components its function fetches, and an LM's token loop at any size.
pub(crate) fn reads(slot: &Slot) -> Reads<'_> {
    Reads { components: &slot.components, any_size: slot.phase.as_deref() == Some("decode") }
}

/// Why a model slot can't load `facts`, whose default is `default`: none when it can (cl-943). Its family label plays no part.
pub(crate) fn unloadable(slot: &Slot, default: &Facts, facts: &Facts) -> Option<String> {
    match facts.lora {
        true => Some("it is a LoRA".into()),
        false => default.parts.lacking(&facts.parts, reads(slot)),
    }
}

/// Refuses a choice its slot can't load, as headers say (cl-943). A model slot loads a checkpoint that holds what its default holds
/// ([`unloadable`]); a LoRA slot, and each LoRA added to a slot, a LoRA of the family the default's header says. `slots` are a
/// function's own by parameter and its calls' by path, as [`Choice::slot`] names them. A slot with no default, one that reads raw
/// weights, and a header the Hub can't give now are the loader's to judge. `target` names the function in the remedy.
pub(crate) async fn check_choices(api: &Api, hub: &str, target: &str, slots: &[(&str, &Slot)], choices: &[Choice]) -> Result<(), Error> {
    let chosen: Vec<(&Slot, &Choice)> = (choices.iter())
        .filter(|choice| choice.reference.is_some() || !choice.adapters.is_empty())
        .filter_map(|choice| slots.iter().find(|(key, _)| *key == choice.slot).map(|(_, slot)| (*slot, choice)))
        .filter(|(slot, _)| !slot.weights && slot.default.is_some())
        .collect();
    if chosen.is_empty() {
        return Ok(());
    }
    let Ok(origin) = Origin::parse(hub) else { return Ok(()) };
    let Ok(client) = reader(api, &origin) else { return Ok(()) };
    let endpoint = distribution(&client, &origin).await.ok();
    let (client, endpoint, hub) = (&client, endpoint.as_ref(), &origin);
    let result = async {
        for (slot, choice) in chosen {
            let default = slot.default.as_deref().unwrap_or_default();
            let Some((default_card, default_facts)) = held(api, client, endpoint, hub, default).await else { continue };
            let named = &choice.slot;
            let same_family = |want: &str, reference: &str, card: &types::ModelCard, facts: &Facts| match adapts(card, facts) {
                Some(got) if !got.eq_ignore_ascii_case(want) => Err(Error::new(Kind::Validation, "model_choice_family", format!("{named} takes {want} LoRAs; {reference} is for {got}"))),
                _ => Ok(()),
            };
            if let Some(reference) = choice.reference.as_deref()
                && let Some((card, facts)) = held(api, client, endpoint, hub, reference).await
            {
                match (default_facts.lora, facts.lora) {
                    (false, true) => {
                        return Err(Error::new(Kind::Validation, "model_choice_kind", format!("{named} takes a model; {reference} is a LoRA"))
                            .next(format!("cozy run … --lora {named}={reference}")));
                    }
                    (false, false) => {
                        if let Some(why) = unloadable(slot, &default_facts, &facts) {
                            return Err(Error::new(Kind::Validation, "model_choice_unloadable", format!("{named} can't load {reference}: {why}"))
                                .next(format!("cozy model ls --fits {}:{named}", api::names::shown(target))));
                        }
                    }
                    (true, false) => {
                        return Err(Error::new(Kind::Validation, "model_choice_kind", format!("{named} takes a LoRA; {reference} is a model")));
                    }
                    (true, true) => {
                        if let Some(want) = adapts(&default_card, &default_facts) {
                            same_family(&want, reference, &card, &facts)?;
                        }
                        if let Some(why) = default_facts.parts.keeps_lacking(&facts.parts, reads(slot)) {
                            return Err(Error::new(Kind::Validation, "model_choice_unloadable", format!("{named} can't load {reference}: {why}")));
                        }
                    }
                }
            }
            for adapter in &choice.adapters {
                let Some((card, facts)) = held(api, client, endpoint, hub, &adapter.reference).await else { continue };
                if !facts.lora {
                    return Err(Error::new(Kind::Validation, "model_choice_kind", format!("{} is a model, not a LoRA for {named}", adapter.reference))
                        .next(format!("cozy run … --model {named}={}", adapter.reference)));
                }
                if let Some(want) = adapts(&default_card, &default_facts) {
                    same_family(&want, &adapter.reference, &card, &facts)?;
                }
            }
        }
        Ok::<(), Error>(())
    }
    .await;
    save(api);
    result
}

/// Each slot's default, as a run naming it loads it: what a choice for the slot is judged by.
pub(crate) async fn defaults(api: &Api, hub: &Origin, slots: &[Slot]) -> Result<Vec<(Slot, Facts)>, Error> {
    let client = reader(api, hub).map_err(|refused| refused.0)?;
    let endpoint = distribution(&client, hub).await.ok();
    let mut read = vec![];
    for slot in slots {
        let default = slot.default.as_deref().unwrap_or_default();
        let found = held(api, &client, endpoint.as_ref(), hub, default).await;
        let Some((_, facts)) = found else {
            save(api);
            return Err(Error::new(Kind::Unavailable, "model_unreadable", format!("{default}'s header can't be read now")).next(format!("cozy model ls {default}")));
        };
        read.push((slot.clone(), facts));
    }
    save(api);
    Ok(read)
}

/// Each declared slot says the family and kind of its default checkpoint.
pub(crate) async fn families(api: &Api, hub: &str, tools: &mut [Tool]) {
    let defaults: BTreeSet<String> = tools.iter().flat_map(|tool| tool.models.iter().chain(tool.model.iter())).filter_map(|model| model.default.clone()).collect();
    let Ok(origin) = Origin::parse(hub) else { return };
    let fresh = |default: &String| {
        let held = named_so_far().lock().expect("named lock");
        held.get(&format!("{origin} {default}")).filter(|(at, _)| at.elapsed() < NAMED_FOR).map(|(_, model)| model.clone())
    };
    let mut found: HashMap<String, DefaultModel> = defaults.iter().filter_map(|default| Some((default.clone(), fresh(default)?))).collect();
    if found.len() < defaults.len() {
        let Ok(client) = reader(api, &origin) else { return };
        let Ok(endpoint) = distribution(&client, &origin).await else { return };
        let (client, endpoint, origin) = (&client, &endpoint, &origin);
        let unread: Vec<String> = defaults.iter().filter(|default| !found.contains_key(*default)).cloned().collect();
        let reads = unread.into_iter().map(|default| async move {
            let model = named(api, client, endpoint, origin, &default).await.unwrap_or_default();
            (default, model)
        });
        let reads: Vec<(String, DefaultModel)> = stream::iter(reads).buffered(READS).collect().await;
        save(api);
        let mut held = named_so_far().lock().expect("named lock");
        for (default, model) in reads {
            held.insert(format!("{origin} {default}"), (Instant::now(), model.clone()));
            found.insert(default, model);
        }
    }
    for tool in tools {
        for model in tool.models.iter_mut().chain(tool.model.iter_mut()) {
            let facts = model.default.as_ref().and_then(|default| found.get(default));
            model.family = facts.and_then(|facts| facts.family.clone());
            model.kind = facts.and_then(|facts| facts.kind);
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
