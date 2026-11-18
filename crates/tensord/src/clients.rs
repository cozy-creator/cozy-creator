//! What each of this machine's clients calls itself (`v1::CLIENT`, its host name), by its key (`runs::actor_id`): who sent each run,
//! as Status says it (`v1::Queued::client`, cl-1193). Held in memory: a client says it with every request, so a restarted machine
//! hears it again at the next.
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

/// The most clients named at once: a machine has a few (its account's computers, a pod-session's runner).
const CLIENTS: usize = 64;
/// A name's most characters.
const NAME_CHARS: usize = 64;

static NAMES: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Mutex::default);

/// The client whose key is `actor` calls itself `said`: its letters, digits, spaces and `.-_`, at most 64.
pub(crate) fn heard(actor: &str, said: &str) {
    let name: String = said.chars().filter(|c| c.is_ascii_alphanumeric() || " .-_".contains(*c)).take(NAME_CHARS).collect();
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    let mut names = NAMES.lock().unwrap();
    if names.get(actor).is_some_and(|held| held == name) {
        return;
    }
    if names.len() >= CLIENTS && !names.contains_key(actor) {
        let Some(any) = names.keys().next().cloned() else { return };
        names.remove(&any);
    }
    names.insert(actor.to_owned(), name.to_owned());
}

/// What the client whose key is `actor` calls itself; empty: it hasn't said since this machine started.
pub(crate) fn name(actor: &str) -> String {
    NAMES.lock().unwrap().get(actor).cloned().unwrap_or_default()
}
