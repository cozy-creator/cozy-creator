//! The Hub's resources through Calcifer (CALCIFER-API.md "Packages, models, account and orgs"):
//! the account this computer signs in as, orgs, models and packages, read and changed as this
//! computer's device key. Every call names its Hub; absent, the configured one. What takes long is a run (`jobs`).
mod account;
pub(crate) mod after;
pub(crate) mod author;
pub(crate) mod checkpoints;
pub(crate) mod jobs;
mod models;
mod orgs;
mod packages;
mod quantize;
mod weights;

use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::rejection::BytesRejection,
    routing::{delete, get, patch, post, put},
};
use ::api::hub::{Client, DeviceKey};
use api::config::Origin;
use serde::de::DeserializeOwned;

use crate::api::{Api, Refusal, parse};

pub(crate) use account::Codes;

pub(crate) fn routes() -> Router<Arc<Api>> {
    Router::new()
        .route("/v1/account", get(account::show).delete(account::sign_out))
        .route("/v1/account/codes", post(account::send_code))
        .route("/v1/account/codes/{id}", post(account::prove))
        .route("/v1/account/codes/{id}/step", post(account::step))
        .route("/v1/account/name", put(account::name))
        .route("/v1/account/key", put(account::install))
        .route("/v1/orgs/{org}", patch(orgs::edit))
        .route("/v1/orgs/{org}/claim", post(orgs::claim))
        .route("/v1/models", get(models::search))
        .route("/v1/families", get(models::families))
        .route("/v1/models/{org}/{name}", get(models::info).patch(models::edit).delete(models::delete))
        .route("/v1/models/{org}/{name}/releases", post(models::release))
        .route("/v1/models/{org}/{name}/releases/{version}", delete(models::delete_release))
        .route("/v1/models/{org}/{name}/releases/{version}/unpublish", post(models::unpublish))
        .route("/v1/models/{org}/{name}/checkpoints/{checkpoint}", delete(models::delete_checkpoint))
        .route("/v1/models/{org}/{name}/commits/{commit}", delete(models::delete_checkpoint))
        .route("/v1/packages", get(packages::search))
        .route("/v1/packages/{org}/{name}", get(packages::info).delete(packages::delete))
        .route("/v1/packages/{org}/{name}/releases/{release}/yank", post(packages::yank).delete(packages::unyank))
}

/// The Hub `named` names, else this computer's configured one.
pub(crate) fn origin(api: &Api, named: Option<&str>) -> Result<Origin, Refusal> {
    let named = named.map(str::trim).filter(|hub| !hub.is_empty());
    Origin::parse(named.unwrap_or(&api.machines.hubs.default_origin())).map_err(Refusal)
}

/// A failed Hub call: its own code as the cause, and `cozy login` next on a refused credential.
pub(crate) fn refusal(hub: &Origin, error: ::api::hub::Error) -> Refusal {
    Refusal(error.refusal(|| crate::rentals::login(hub)))
}

/// `hub` as this computer's device key; refused `not_logged_in` without one.
fn signed_in(api: &Api, hub: &Origin) -> Result<Client, Refusal> {
    api.machines.hubs.client(hub).map_err(Refusal)
}

/// `hub` signed in when this computer is, else anonymous: what public and owned reads both need.
fn reader(api: &Api, hub: &Origin) -> Result<Client, Refusal> {
    api.machines.hubs.reader(hub).map_err(Refusal)
}

/// The device key a signed-in client speaks as.
fn key(client: &Client) -> &DeviceKey {
    match client.credential() {
        ::api::hub::Credential::User(session) => session.key(),
        _ => unreachable!("a signed-in client"),
    }
}

/// A body that may be empty (an action that needs nothing but its Hub).
pub(crate) fn body<T: DeserializeOwned + Default>(body: Result<Bytes, BytesRejection>) -> Result<T, Refusal> {
    let body = body?;
    match body.iter().all(u8::is_ascii_whitespace) {
        true => Ok(T::default()),
        false => parse(&body),
    }
}
