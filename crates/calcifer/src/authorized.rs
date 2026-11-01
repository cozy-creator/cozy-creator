//! `/v1/machines/local/authorized-keys`: the controllers of other computers this computer's
//! machine admits (`cozy machine authorize`). While any is authorized the machine listens on every
//! interface (it moves there once idle), stays up, and Calcifer starts it with itself at login.
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State},
};
use protocol::{
    calcifer::{Authorized, AuthorizedKeys},
    error::{Error, Kind},
};

use crate::{
    api::{Api, Refusal},
    machines::Machines,
};

impl Machines {
    /// Adds (`add`) or takes back a controller's key. `local` reads its keys again at its next
    /// call, which this makes: once idle it listens on every interface, and `reach` then says
    /// where other computers add it.
    pub async fn authorize(self: &Arc<Self>, key: &str, add: bool) -> Result<Authorized, Error> {
        protocol::calcifer::local_usable("local")?;
        let changed = self.local.authorize(key, add)?;
        let image = self.local.image();
        let machine = self.show("local").await?;
        Ok(Authorized {
            key: key.to_owned(),
            file: self.local.keys_file(),
            changed,
            machine,
            image,
        })
    }
}

fn only_local(name: &str) -> Result<(), Refusal> {
    match name {
        "local" => Ok(()),
        _ => Err(Refusal(
            Error::new(
                Kind::Usage,
                "authorized_keys_local_only",
                format!("{name} keeps its own keys: only this computer's machine is authorized here"),
            )
            .next("on that computer: cozy machine authorize <key>"),
        )),
    }
}

pub(crate) async fn list(
    State(api): State<Arc<Api>>,
    UrlPath(name): UrlPath<String>,
) -> Result<Json<AuthorizedKeys>, Refusal> {
    only_local(&name)?;
    let local = &api.machines.local;
    Ok(Json(AuthorizedKeys {
        file: local.keys_file(),
        keys: local.keys().map_err(Refusal)?,
    }))
}

pub(crate) async fn add(
    State(api): State<Arc<Api>>,
    UrlPath((name, key)): UrlPath<(String, String)>,
) -> Result<Json<Authorized>, Refusal> {
    only_local(&name)?;
    Ok(Json(api.machines.authorize(key.trim(), true).await.map_err(Refusal)?))
}

pub(crate) async fn remove(
    State(api): State<Arc<Api>>,
    UrlPath((name, key)): UrlPath<(String, String)>,
) -> Result<Json<Authorized>, Refusal> {
    only_local(&name)?;
    Ok(Json(api.machines.authorize(key.trim(), false).await.map_err(Refusal)?))
}
