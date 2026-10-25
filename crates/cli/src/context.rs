//! What every verb receives: the global flags, the one loaded config, the terminal's facts and
//! the Hub clients, which share one HTTP client.
use std::{
    fmt::Debug,
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use platform::paths::Home;
use protocol::{
    config::{Config, Loaded, Origin},
    error::Error,
};
use hub_client::{Credential, ResponseValue, UserSession};

use crate::{calcifer_client::Client, output::Term};

#[derive(Debug)]
pub struct Context {
    pub json: bool,
    /// `--yes`: agreed without being asked.
    pub yes: bool,
    pub config: Config,
    pub home: Home,
    /// The user's home directory: services and `~` are relative to it.
    pub user_home: PathBuf,
    pub term: Term,
    http: OnceLock<reqwest::Client>,
}

impl Context {
    pub fn new(json: bool, yes: bool, loaded: Loaded, term: Term) -> Self {
        Self {
            json,
            yes,
            config: loaded.config,
            home: loaded.home,
            user_home: loaded.user_home,
            term,
            http: OnceLock::new(),
        }
    }

    /// The Hub: `COZY_HUB_URL`, else config.toml's `hub_url`, else tensorhub.com.
    pub fn hub(&self) -> &Origin {
        &self.config.hub_url
    }

    /// `origin` when it names a Hub other than this one: a record left from another.
    pub fn other_hub<'a>(&self, origin: &'a str) -> Option<&'a str> {
        Origin::parse(origin).is_ok_and(|o| o != *self.hub()).then_some(origin)
    }

    /// Calcifer, started if none runs.
    pub async fn calcifer(&self) -> Result<Client, Error> {
        Client::connect(self).await
    }

    /// Whether this computer holds a device key for `hub`: signed in, as far as the files say.
    pub fn signed_in(&self, hub: &Origin) -> bool {
        hub.device_key(&self.home).exists()
    }

    /// The one HTTP client this command's Hub calls share.
    pub fn http(&self) -> &reqwest::Client {
        self.http.get_or_init(reqwest::Client::new)
    }

    pub fn hub_anonymous(&self, hub: &Origin) -> hub_client::Client {
        hub_client::Client::connect(hub, self.http().clone(), Credential::Anonymous)
    }

    /// `hub` as this computer's signed-in user; refused when it has no device key there.
    pub fn hub_signed_in(&self, hub: &Origin) -> Result<hub_client::Client, Error> {
        hub_client::Client::signed_in(hub, &self.home, self.http().clone(), || {
            self.config.login(hub)
        })
    }

    /// This computer's device key at `hub` and the Bearer it mints, kept beside it.
    pub fn hub_session(&self, hub: &Origin) -> Result<Arc<UserSession>, Error> {
        UserSession::open(hub, &self.home, || self.config.login(hub)).map(Arc::new)
    }

    /// Signed in when this computer is, else anonymous: what public and owned reads both need.
    pub fn hub_client(&self, hub: &Origin) -> Result<hub_client::Client, Error> {
        match self.signed_in(hub) {
            true => self.hub_signed_in(hub),
            false => Ok(self.hub_anonymous(hub)),
        }
    }
}


/// A Hub call's answer, or its refusal as the CLI's own.
pub fn hub_answer<T, E>(
    answer: Result<ResponseValue<T>, progenitor_client::Error<E>>,
) -> Result<T, Error>
where
    E: serde::Serialize + Debug,
{
    answer.map(ResponseValue::into_inner).map_err(hub_error)
}

/// A failed Hub call as the CLI's refusal.
pub fn hub_error(error: impl Into<hub_client::Error>) -> Error {
    error.into().refusal(|| "cozy login <email>".into())
}
