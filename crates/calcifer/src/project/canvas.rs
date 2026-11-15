//! Where a project's boxes sit: `project.db`'s `canvas` rows. Only the page reads them, and nothing in them is an input. A row says the
//! revision and the client that last wrote it, so a place the page worked out from an older read can't clobber another's newer one (cl-800),
//! checked inside the write's own transaction. Losing them loses only the arrangement.
use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
    http::StatusCode,
};
use api::{
    error::Error,
    calcifer::project::{Canvas, CanvasPatch, CanvasPatched, Card},
};

use super::{db::CardRow, folder::Folder, graph::name_key};
use crate::api::{Api, Refusal, parse};

fn number(value: f64) -> f64 {
    (value * 100.0).round() / 100.0 + 0.0
}

/// A card as a row holds it: to a hundredth, a width only if it is one.
fn tidied(card: Card) -> Card {
    Card { x: number(card.x), y: number(card.y), w: card.w.filter(|w| *w > 0.0).map(number) }
}

/// These rows set, a none gone, and told to every stream once written. A name is one row in any case, with `_` and `-` alike.
pub(super) fn set(folder: &Folder, id: &str, changed: BTreeMap<String, Option<Card>>) -> Result<(), Error> {
    set_from(folder, id, changed, &BTreeMap::new()).map(drop)
}

/// As `set`, but a row in `placed`, a client's own working, is set only if no other client wrote it after the revision `placed` gives it,
/// what it was worked out from: one that was is left as it is, and answered with what it is.
fn set_from(folder: &Folder, id: &str, changed: BTreeMap<String, Option<Card>>, placed: &BTreeMap<String, u64>) -> Result<BTreeMap<String, Option<Card>>, Error> {
    let by = super::history::by_now();
    let placed: BTreeMap<String, u64> = placed.iter().map(|(name, since)| (name_key(name), *since)).collect();
    folder.act(|| {
        let (applied, kept, was_there) = folder.edit_canvas(|rows| {
            let mut kept = BTreeMap::new();
            let mut applied = BTreeMap::new();
            let mut was_there = vec![];
            for (name, card) in changed {
                let key = name_key(&name);
                let now = rows.get(&key);
                let stands = placed.get(&key).is_some_and(|since| now.is_some_and(|row| row.by != by && row.seq > *since));
                if stands {
                    kept.insert(name, now.map(|row| row.card));
                    continue;
                }
                let card = card.map(tidied);
                was_there.push((name.clone(), now.map(|row| row.card), card));
                match card {
                    Some(card) => drop(rows.insert(key, CardRow { name: name.clone(), card, seq: 0, by: by.clone() })),
                    None => drop(rows.remove(&key)),
                }
                applied.insert(name, card);
            }
            Ok((applied, kept, was_there))
        })?;
        super::history::cards(folder, was_there);
        if !applied.is_empty() {
            let id = id.to_owned();
            folder.then(move |seq| crate::events::canvas(&id, seq, applied));
        }
        Ok(kept)
    })
}

/// `name`'s row, if it has one.
pub(super) fn row(folder: &Folder, name: &str) -> Result<Option<Card>, Error> {
    Ok(folder.canvas()?.get(&name_key(name)).map(|row| row.card))
}

/// `from`'s row as `to`'s.
pub(super) fn moved(folder: &Folder, id: &str, from: &str, to: &str) -> Result<(), Error> {
    let Some(card) = row(folder, from)? else { return Ok(()) };
    set(folder, id, BTreeMap::from([(from.to_owned(), None), (to.to_owned(), Some(card))]))
}

/// The rows `named` says nothing names, gone.
pub(super) fn prune(folder: &Folder, id: &str, named: impl Fn(&str) -> bool) -> Result<(), Error> {
    let gone: BTreeMap<String, Option<Card>> = folder.canvas()?.into_values().filter(|row| !named(&row.name)).map(|row| (row.name, None)).collect();
    if gone.is_empty() {
        return Ok(());
    }
    set(folder, id, gone)
}

/// `GET /v1/projects/{id}/canvas`: each row, and the revision they're of.
pub(super) async fn read(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<Canvas>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let (seq, rows) = project.folder.canvas_at().map_err(Refusal)?;
    Ok(Json(Canvas { seq, cards: rows.into_values().map(|row| (row.name, row.card)).collect() }))
}

/// `PATCH /v1/projects/{id}/canvas`: last move wins, but for a row the page placed itself, which stands aside for another's newer one.
pub(super) async fn patch(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<CanvasPatched>, Refusal> {
    let asked: CanvasPatch = parse(&body?)?;
    // Only rows it placed itself: the page arranging its own, which no one undoes.
    if asked.cards.keys().all(|name| asked.placed.keys().any(|placed| name_key(placed) == name_key(name))) {
        super::history::quiet();
    }
    let project = api.projects.get(&api, &id)?;
    let id = project.id.clone();
    let folder = &project.folder;
    // Its answer is kept with it, in its transaction, at the revision it commits (cl-885).
    let kept = folder.act(|| {
        let kept = set_from(folder, &id, asked.cards, &asked.placed)?;
        let said = kept.clone();
        super::keyed::answering_at(StatusCode::OK, move |seq, _| CanvasPatched { seq, kept: said.clone() });
        Ok(kept)
    }).map_err(Refusal)?;
    Ok(Json(CanvasPatched { seq: folder.seq(), kept }))
}

/// Rows a request makes beside its box, as one transaction with it: the push-aside of a drop (`moves`, each worked out from its revision in
/// `placed`; one it doesn't give is the client's to set).
pub(super) fn pushed(folder: &Folder, id: &str, moves: BTreeMap<String, Card>, placed: &BTreeMap<String, u64>) -> Result<BTreeMap<String, Option<Card>>, Error> {
    if moves.is_empty() {
        return Ok(BTreeMap::new());
    }
    set_from(folder, id, moves.into_iter().map(|(name, card)| (name, Some(card))).collect(), placed)
}
