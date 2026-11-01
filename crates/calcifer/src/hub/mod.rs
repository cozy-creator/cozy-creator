//! The Hub's resources through Calcifer (CALCIFER-API.md "Packages, models, account and orgs"):
//! the account this computer signs in as, orgs, models and packages, read and changed as this
//! computer's device key. Every call names its Hub; absent, the configured one.
mod account;
pub(crate) mod after;
mod asks;
mod author;
mod models;
mod orgs;
mod packages;
mod quantize;
mod weights;
mod work;

use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::rejection::BytesRejection,
    routing::{delete, get, patch, post, put},
};
use hub_client::{Client, DeviceKey};
use protocol::{
    config::Origin,
    hub::{ModelDownload, ModelUpload, PackageInstall},
};
use serde::de::DeserializeOwned;

use crate::api::{Api, Refusal, parse};

pub(crate) use account::Codes;
pub(crate) use work::Works;

pub fn routes() -> Router<Arc<Api>> {
    Router::new()
        .route("/v1/account", get(account::show).delete(account::sign_out))
        .route("/v1/account/codes", post(account::send_code))
        .route("/v1/account/codes/{id}", post(account::prove))
        .route("/v1/account/name", put(account::name))
        .route("/v1/account/key", put(account::install))
        .route("/v1/orgs/{org}", patch(orgs::edit))
        .route("/v1/orgs/{org}/claim", post(orgs::claim))
        .route("/v1/models", get(models::search))
        .route("/v1/models/{org}/{name}", get(models::info).patch(models::edit).delete(models::delete))
        .route("/v1/models/{org}/{name}/releases", post(models::release))
        .route("/v1/models/{org}/{name}/releases/{version}", delete(models::delete_release))
        .route("/v1/models/{org}/{name}/commits", post(weights::record))
        .route("/v1/models/{org}/{name}/commits/{commit}", delete(models::delete_commit))
        .route("/v1/model-downloads", post(asks::create::<ModelDownload>))
        .route("/v1/model-download-quotes", post(asks::quote::<ModelDownload>))
        .route("/v1/model-uploads", post(weights::upload))
        .route("/v1/model-upload-quotes", post(asks::quote::<ModelUpload>))
        .route("/v1/model-quantizations", post(quantize::create))
        .route("/v1/model-quantization-quotes", post(quantize::quote))
        .route("/v1/package-installs", post(asks::create::<PackageInstall>))
        .route("/v1/package-install-quotes", post(asks::quote::<PackageInstall>))
        .route("/v1/package-removals", post(packages::remove))
        .route("/v1/package-locks", post(work::lock))
        .route("/v1/package-locks/{id}", get(work::show_lock))
        .route("/v1/package-locks/{id}/events", get(work::lock_events))
        .route("/v1/package-publications", post(work::publish))
        .route("/v1/package-publications/{id}", get(work::show_publication))
        .route("/v1/package-publications/{id}/events", get(work::publication_events))
        .route("/v1/packages", get(packages::search))
        .route("/v1/packages/{org}/{name}", get(packages::info).delete(packages::delete))
        .route("/v1/packages/{org}/{name}/releases/{release}/yank", post(packages::yank).delete(packages::unyank))
}

/// The Hub `named` names, else this computer's configured one.
fn origin(api: &Api, named: Option<&str>) -> Result<Origin, Refusal> {
    let named = named.map(str::trim).filter(|hub| !hub.is_empty());
    Origin::parse(named.unwrap_or(&api.machines.hubs.default_origin())).map_err(Refusal)
}

/// A failed Hub call: its own code as the cause, and `cozy login` next on a refused credential.
fn refusal(hub: &Origin, error: hub_client::Error) -> Refusal {
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
        hub_client::Credential::User(session) => session.key(),
        _ => unreachable!("a signed-in client"),
    }
}

/// A body that may be empty (an action that needs nothing but its Hub).
fn body<T: DeserializeOwned + Default>(body: Result<Bytes, BytesRejection>) -> Result<T, Refusal> {
    let body = body?;
    match body.iter().all(u8::is_ascii_whitespace) {
        true => Ok(T::default()),
        false => parse(&body),
    }
}
