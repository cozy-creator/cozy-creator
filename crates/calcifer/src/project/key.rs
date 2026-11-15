//! A project's key (cl-987, as Jira's): what its runs are named by, `KEY-N`, N its number there. Proposed from its name as it is first
//! opened here, unique among the projects this computer knows, and kept in its project.db, so it goes with the folder and outlives a
//! rename. A key it had names its runs still.
use std::sync::Arc;

use api::{
    calcifer::project::{self as wire, Key},
    error::{Error, Kind},
};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
};

use super::Project;
use crate::api::{Api, Refusal};

/// The keys `name` proposes, best first: its words' initials (`Teams in Space` is `TIS`), a one-word name's first three letters
/// (`Marketing` is `MAR`); then a letter more of its last word each (`TISP`, `MARK`), then numbered (`TIS2`).
pub(crate) fn proposed(name: &str) -> impl Iterator<Item = String> {
    let words: Vec<Vec<u8>> = name.split(|c: char| !c.is_ascii_alphabetic()).filter(|word| !word.is_empty()).map(|word| word.to_ascii_uppercase().into_bytes()).collect();
    let (mut base, more): (Vec<u8>, Vec<u8>) = match words.as_slice() {
        [] => (b"PROJ".to_vec(), vec![]),
        [word] => (word[..word.len().min(3)].to_vec(), word[word.len().min(3)..].to_vec()),
        [.., last] => (words.iter().take(10).map(|word| word[0]).collect(), last[1..].to_vec()),
    };
    let mut more = more.into_iter();
    while base.len() < 2 {
        base.push(more.next().unwrap_or(b'X'));
    }
    let longer: Vec<String> = std::iter::successors(Some(base.clone()), |held| {
        let next = more.next().filter(|_| held.len() < 10)?;
        Some([held.as_slice(), &[next]].concat())
    })
    .map(|key| String::from_utf8(key).expect("ASCII"))
    .collect();
    let stem = String::from_utf8(base).expect("ASCII");
    let numbered = (2u32..).map(move |n| {
        let n = n.to_string();
        format!("{}{n}", &stem[..stem.len().min(10 - n.len())])
    });
    longer.into_iter().chain(numbered)
}

impl Project {
    pub(crate) fn key(&self) -> String {
        self.key.lock().expect("key lock").clone()
    }
}

fn store_error(error: crate::store::StoreError) -> Error {
    match error {
        crate::store::StoreError::ProjectKey(message) => Error::new(Kind::Conflict, "project_key_taken", message).next("cozy key <KEY>"),
        other => Error::internal("calcifer_store", other.to_string()),
    }
}

/// The key project `id`, called `name`, goes by here, given the keys its project.db keeps (`held`, its key now last).
pub(super) fn keyed(api: &Api, id: &str, name: &str, held: &[String]) -> Result<String, Error> {
    api.calcifer.store().project_keyed(id, held, proposed(name)).map_err(store_error)
}

fn answer(api: &Api, project: &Project) -> Result<Key, Error> {
    let former = api.calcifer.store().former_keys(&project.id).map_err(store_error)?;
    Ok(Key { key: project.key(), former })
}

/// `GET /v1/projects/{id}/key`.
pub(super) async fn read(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<Key>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    Ok(Json(answer(&api, &project).map_err(Refusal)?))
}

/// `PUT /v1/projects/{id}/key` (`{key}`): its key is that now, kept here and in its project.db; the one it had names its runs still.
pub(super) async fn change(State(api): State<Arc<Api>>, Path(id): Path<String>, body: Bytes) -> Result<Json<Key>, Refusal> {
    let asked: Key = crate::api::parse(&body)?;
    let key = wire::key(&asked.key).ok_or_else(|| {
        Refusal(Error::new(Kind::Validation, "project_key_invalid", format!("{:?} is no project key: 2 to 10 letters and digits, a letter first", asked.key))
            .remedy("for example SE or SHOTS2"))
    })?;
    let project = api.projects.get(&api, &id)?;
    if key != project.key() {
        tokio::task::block_in_place(|| {
            api.calcifer.store().project_key_changed(&project.id, &key).map_err(store_error)?;
            project.folder.set_key(&key, false)
        }).map_err(Refusal)?;
        *project.key.lock().expect("key lock") = key;
    }
    Ok(Json(answer(&api, &project).map_err(Refusal)?))
}
