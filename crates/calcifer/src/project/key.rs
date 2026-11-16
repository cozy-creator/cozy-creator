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

/// The keys `name` proposes, best first (cl-1075): its words' initials (`Teams in Space` is `TIS`), a one-word name's first three
/// letters (`Marketing` is `MAR`), each followed by the name's digits (`lab2` is `LAB2`, `lab3_2` is `LAB32`); then a letter more of its
/// last word each (`TISP`, `MARK`), then letters after it (`LAB32A`, `LAB32B`, … `LAB32AA`). Never a digit more than the name has, so a
/// key taken twice never reads as another name.
pub(crate) fn proposed(name: &str) -> impl Iterator<Item = String> {
    let words: Vec<Vec<u8>> = name.split(|c: char| !c.is_ascii_alphabetic()).filter(|word| !word.is_empty()).map(|word| word.to_ascii_uppercase().into_bytes()).collect();
    let digits: Vec<u8> = name.bytes().filter(u8::is_ascii_digit).collect();
    let (mut base, more): (Vec<u8>, Vec<u8>) = match words.as_slice() {
        [] => (b"PROJ".to_vec(), vec![]),
        [word] => (word[..word.len().min(3)].to_vec(), word[word.len().min(3)..].to_vec()),
        [.., last] => (words.iter().take(10).map(|word| word[0]).collect(), last[1..].to_vec()),
    };
    let mut more = more.into_iter();
    while base.len() + digits.len() < 2 {
        base.push(more.next().unwrap_or(b'X'));
    }
    // A stem, then as many of the digits as fit, then `tail` whole: a key of ten at most.
    let keyed = move |stem: &[u8], tail: &[u8]| -> String {
        let room = 10 - tail.len();
        let stem = &stem[..stem.len().min(room).max(1)];
        let digits = &digits[..digits.len().min(room - stem.len())];
        String::from_utf8([stem, digits, tail].concat()).expect("ASCII")
    };
    let longer: Vec<Vec<u8>> = std::iter::successors(Some(base.clone()), |held| {
        let next = more.next().filter(|_| held.len() < 10)?;
        Some([held.as_slice(), &[next]].concat())
    })
    .collect();
    // A, …, Z, AA, …: n in base 26.
    let lettered = (1u64..).map(|mut n| {
        let mut tail = vec![];
        while n > 0 {
            n -= 1;
            tail.insert(0, b'A' + (n % 26) as u8);
            n /= 26;
        }
        tail
    });
    let first = keyed.clone();
    longer.into_iter().map(move |stem| first(&stem, &[])).chain(lettered.map(move |tail| keyed(&base, &tail)))
}

/// What a project named `name`, or in a folder named so, is proposed first: its name's own key.
pub(crate) fn name_key(name: &str) -> String {
    proposed(name).next().expect("a name proposes a key")
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

/// The key project `id`, called `name`, in `dir`, goes by here, given the keys its project.db keeps (`held`, its key now last), and
/// whether those wait to be looked at again (`review`, cl-1075): one another known folder's name proposes first, and its own name
/// doesn't, goes then. True: it goes by a new key alone, nothing names it by those it had.
pub(super) fn keyed(api: &Api, id: &str, name: &str, dir: &std::path::Path, held: &[String], review: bool) -> Result<(String, bool), Error> {
    let own = [Some(name_key(name)), dir.file_name().map(|folder| name_key(&folder.to_string_lossy()))];
    let own: Vec<String> = own.into_iter().flatten().collect();
    api.calcifer.store().project_keyed(id, dir, held, &own, review, proposed(name)).map_err(store_error)
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
