//! What every verb receives: the global flags, the one loaded config and the terminal's facts. The
//! Hub is Calcifer's to call (CALCIFER-API.md); a verb names it, and nothing here speaks to it.
use std::path::PathBuf;

use platform::paths::Home;
use api::{
    config::{Config, Loaded, Origin},
    error::Error,
};

use crate::{calcifer_client::Client, output::Term};

#[derive(Debug)]
pub(crate) struct Context {
    pub json: bool,
    /// `--full`: the whole record, where a view shows part of it.
    pub full: bool,
    /// `--yes`: agreed without being asked.
    pub yes: bool,
    pub config: Config,
    pub home: Home,
    /// The user's home directory: services and `~` are relative to it.
    pub user_home: PathBuf,
    pub term: Term,
}

impl Context {
    pub(crate) fn new(json: bool, full: bool, yes: bool, loaded: Loaded, term: Term) -> Self {
        Self {
            json,
            full,
            yes,
            config: loaded.config,
            home: loaded.home,
            user_home: loaded.user_home,
            term,
        }
    }

    /// The Hub: `COZY_HUB_URL`, else config.toml's `hub_url`, else tensorhub.com.
    pub(crate) fn hub(&self) -> &Origin {
        &self.config.hub_url
    }

    /// `origin` when it names a Hub other than this one: a record left from another.
    pub(crate) fn other_hub<'a>(&self, origin: &'a str) -> Option<&'a str> {
        Origin::parse(origin).is_ok_and(|o| o != *self.hub()).then_some(origin)
    }

    /// Calcifer, started if none runs.
    pub(crate) async fn calcifer(&self) -> Result<Client, Error> {
        Client::connect(self).await
    }
}
