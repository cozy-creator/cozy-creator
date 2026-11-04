//! `project.canvas.yaml`: where a project's elements sit. Only the page reads it, and `Doc::read` never opens it, so nothing in it is an
//! input. One line per card, sorted by name, so git merges it, and read a line at a time, so a merge's markers cost only themselves.
//! Losing it loses only the arrangement.
use std::{collections::BTreeMap, fmt::Write as _, sync::Arc};

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

use super::{folder::Folder, format::yaml_key, graph::name_key};
use crate::api::{Api, Refusal, parse};

const HEAD: &str = "# Where cards sit. Only the page reads this. Nothing here is an input.\nversion: 1\ncards:\n";

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
    let changed: BTreeMap<String, Option<Card>> = changed.into_iter().map(|(name, card)| (name, card.map(tidied))).collect();
    folder.edit_canvas(|text| {
        let mut cards = rows(text);
        cards.retain(|there, _| !changed.keys().any(|name| name_key(there) == name_key(name)));
        cards.extend(changed.iter().filter_map(|(name, card)| Some((name.clone(), (*card)?))));
        Ok(if cards.is_empty() && text.is_empty() { String::new() } else { written(&cards) })
    })?;
    let etag = etag(&folder.canvas_text()?);
    crate::events::canvas(id, &etag, changed);
    Ok(etag)
}

/// `name`'s row, if it has one.
pub(super) fn row(folder: &Folder, name: &str) -> Result<Option<Card>, Error> {
    Ok(rows(&folder.canvas_text()?).into_iter().find(|(there, _)| name_key(there) == name_key(name)).map(|(_, card)| card))
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

/// `PATCH /v1/projects/{id}/canvas`: last move wins.
pub(super) async fn patch(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<CanvasPatched>, Refusal> {
    let asked: CanvasPatch = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let etag = set(&project.folder, &id, asked.cards).map_err(Refusal)?;
    Ok(Json(CanvasPatched { etag }))
}
