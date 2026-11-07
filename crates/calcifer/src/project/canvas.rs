//! `project.canvas.yaml`: where a project's elements sit. Only the page reads it, and `Doc::read` never opens it, so nothing in it is an
//! input. One line per card, sorted by name, so git merges it, and read a line at a time, so a merge's markers cost only themselves.
//! Losing it loses only the arrangement.
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Write as _,
    path::Path as FsPath,
    sync::{Arc, LazyLock, Mutex},
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
};
use protocol::{
    error::Error,
    project::{Canvas, CanvasPatch, CanvasPatched, Card},
};
use sha2::{Digest as _, Sha256};

use super::{
    folder::{CANVAS, Folder},
    format::yaml_key,
    graph::name_key,
    project_id, stamp,
};
use crate::api::{Api, Refusal, parse};

const HEAD: &str = "# Where cards sit. Only the page reads this. Nothing here is an input.\nversion: 1\ncards:\n";

/// Each project's rows as last told, and what its file's stats said then: a look reads a file only once they say another's there.
static TOLD: LazyLock<Mutex<HashMap<String, (String, BTreeMap<String, Card>)>>> = LazyLock::new(Mutex::default);

/// Each row `text` holds; what doesn't read as one is left out.
fn rows(text: &str) -> BTreeMap<String, Card> {
    let row = |line: &str| {
        let line = protocol::yaml::to_json(line.trim()).ok()?;
        let (name, card) = line.as_object()?.iter().next()?;
        Some((name.clone(), serde_json::from_value(card.clone()).ok()?))
    };
    text.lines().filter(|line| line.starts_with(' ')).filter_map(row).collect()
}

fn number(value: f64) -> f64 {
    (value * 100.0).round() / 100.0 + 0.0
}

/// A card as the file writes it: to a hundredth, a width only if it is one.
fn tidied(card: Card) -> Card {
    Card { x: number(card.x), y: number(card.y), w: card.w.filter(|w| *w > 0.0).map(number) }
}

fn written(cards: &BTreeMap<String, Card>) -> String {
    let mut text = HEAD.to_owned();
    for (name, card) in cards {
        let width = card.w.map_or(String::new(), |w| format!(", w: {w}"));
        writeln!(text, "  {}: {{x: {}, y: {}{width}}}", yaml_key(name), card.x, card.y).expect("a string takes it");
    }
    text
}

fn etag(text: &str) -> String {
    format!("\"{}\"", hex::encode(Sha256::digest(text)))
}

/// These rows set, a none gone, over what the file holds this moment, and told to every stream: the new ETag. A name is one row in
/// any case, with `_` and `-` alike.
pub(super) fn set(folder: &Folder, id: &str, changed: BTreeMap<String, Option<Card>>) -> Result<String, Error> {
    set_from(folder, id, changed, &BTreeMap::new()).map(|(etag, _)| etag)
}

/// As `set`, but a name in `was` is set only if its row is still what `was` says (a none: none), in the same write: one that moved is
/// left as it is, and answered with what it is.
fn set_from(folder: &Folder, id: &str, changed: BTreeMap<String, Option<Card>>, was: &BTreeMap<String, Option<Card>>) -> Result<(String, BTreeMap<String, Option<Card>>), Error> {
    let changed: BTreeMap<String, Option<Card>> = changed.into_iter().map(|(name, card)| (name, card.map(tidied))).collect();
    let now = RefCell::new((BTreeMap::new(), String::new(), BTreeMap::new(), BTreeMap::new()));
    let at = folder.dir.join(CANVAS);
    let held = RefCell::new(TOLD.lock().expect("told lock").remove(id));
    folder.edit_canvas(|text| {
        // What was told of this very file is what it holds: at thousands of rows, reading them again would be most of a move.
        let told = held.borrow_mut().take().filter(|(read, _)| *read == stamp(&at));
        let mut cards = told.map_or_else(|| rows(text), |(_, cards)| cards);
        let there = |cards: &BTreeMap<String, Card>, name: &str| cards.iter().find(|(held, _)| name_key(held) == name_key(name)).map(|(_, card)| *card);
        let kept: BTreeMap<String, Option<Card>> = (was.iter())
            .filter(|(name, from)| changed.contains_key(*name) && there(&cards, name) != from.map(tidied))
            .map(|(name, _)| (name.clone(), there(&cards, name)))
            .collect();
        let applied: BTreeMap<String, Option<Card>> = changed.iter().filter(|(name, _)| !kept.contains_key(*name)).map(|(name, card)| (name.clone(), *card)).collect();
        let keys: HashSet<String> = applied.keys().map(|name| name_key(name)).collect();
        cards.retain(|there, _| !keys.contains(&name_key(there)));
        cards.extend(applied.iter().filter_map(|(name, card)| Some((name.clone(), (*card)?))));
        let written = if cards.is_empty() && text.is_empty() { String::new() } else { written(&cards) };
        *now.borrow_mut() = (cards, written.clone(), applied, kept);
        Ok(written)
    })?;
    let (cards, text, applied, kept) = now.into_inner();
    let etag = etag(&text);
    TOLD.lock().expect("told lock").insert(id.to_owned(), (stamp(&at), cards));
    if !applied.is_empty() {
        crate::events::canvas(id, &etag, applied);
    }
    Ok((etag, kept))
}

/// Tells every stream what changed of `dir`'s rows outside Calcifer: a hand, or a `git pull`.
pub(super) fn look(dir: &FsPath) {
    let id = project_id(dir);
    let stamped = stamp(&dir.join(CANVAS));
    let mut told = TOLD.lock().expect("told lock");
    if told.get(&id).is_some_and(|(held, _)| *held == stamped) {
        return;
    }
    let Ok(text) = Folder::at(dir).canvas_text() else { return };
    let now = rows(&text);
    let Some((_, was)) = told.insert(id.clone(), (stamped, now.clone())) else { return };
    let gone = was.keys().filter(|name| !now.contains_key(*name)).map(|name| (name.clone(), None));
    let changed: BTreeMap<String, Option<Card>> = now.iter().filter(|(name, card)| was.get(*name) != Some(card)).map(|(name, card)| (name.clone(), Some(*card))).chain(gone).collect();
    if !changed.is_empty() {
        crate::events::canvas(&id, &etag(&text), changed);
    }
}

/// `name`'s row, if it has one.
pub(super) fn row(folder: &Folder, name: &str) -> Result<Option<Card>, Error> {
    let of = |cards: &BTreeMap<String, Card>| cards.get(name).or_else(|| cards.iter().find(|(there, _)| name_key(there) == name_key(name)).map(|(_, card)| card)).copied();
    let id = project_id(&folder.dir);
    if let Some((read, cards)) = TOLD.lock().expect("told lock").get(&id)
        && *read == stamp(&folder.dir.join(CANVAS))
    {
        return Ok(of(cards));
    }
    Ok(of(&rows(&folder.canvas_text()?)))
}

/// `from`'s row as `to`'s.
pub(super) fn moved(folder: &Folder, id: &str, from: &str, to: &str) -> Result<(), Error> {
    let Some(card) = row(folder, from)? else { return Ok(()) };
    set(folder, id, BTreeMap::from([(from.to_owned(), None), (to.to_owned(), Some(card))])).map(drop)
}

/// Every row as `renamed` says, with nothing told, for what no Calcifer is running.
pub(super) fn all_renamed(folder: &Folder, renamed: impl Fn(&str) -> Option<String>) -> Result<(), Error> {
    folder.edit_canvas(|text| {
        let cards: BTreeMap<String, Card> = rows(text).into_iter().map(|(name, card)| (renamed(&name).unwrap_or(name), card)).collect();
        Ok(if cards.is_empty() && text.is_empty() { String::new() } else { written(&cards) })
    })
}

/// The rows `named` says nothing names, gone.
pub(super) fn prune(folder: &Folder, id: &str, named: impl Fn(&str) -> bool) -> Result<(), Error> {
    let gone: BTreeMap<String, Option<Card>> = rows(&folder.canvas_text()?).into_keys().filter(|name| !named(name)).map(|name| (name, None)).collect();
    if gone.is_empty() {
        return Ok(());
    }
    set(folder, id, gone).map(drop)
}

/// `GET /v1/projects/{id}/canvas`.
pub(super) async fn read(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<Canvas>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let text = project.folder.canvas_text().map_err(Refusal)?;
    Ok(Json(Canvas { etag: etag(&text), cards: rows(&text) }))
}

/// `PATCH /v1/projects/{id}/canvas`: last move wins, but for a row asked only if it's still what it was.
pub(super) async fn patch(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<CanvasPatched>, Refusal> {
    let asked: CanvasPatch = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let (etag, kept) = set_from(&project.folder, &id, asked.cards, &asked.was).map_err(Refusal)?;
    Ok(Json(CanvasPatched { etag, kept }))
}
