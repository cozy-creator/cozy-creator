//! Where objects come from. An origin says where its objects can be read (`locate`); the Fetcher
//! downloads, verifies and admits them.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use super::token::TokenProvider;
use crate::err::Result;
use crate::format::ObjectRef;

/// What an async method of a `dyn` trait returns: `async fn` in a trait object is boxed by hand.
pub type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Origin: Send + Sync {
    /// Where each of `objects` can be read, in order. The Fetcher asks once per object as it
    /// starts it; an origin whose answer costs a round trip batches and caches.
    fn locate<'a>(&'a self, objects: &'a [ObjectRef]) -> Boxed<'a, Result<Vec<Location>>>;

    /// Whether anyone may read these objects: only then are they written back to a shared
    /// volume.
    fn public(&self) -> bool;
}

/// Where one object can be read.
#[derive(Clone)]
pub enum Location {
    /// An HTTP(S) GET, with the headers it needs.
    Url { url: String, headers: Vec<(String, String)> },
    /// A provider presents each GET afresh, including same-origin redirects and restarts.
    Authenticated {
        url: String,
        provider: Arc<dyn TokenProvider>,
    },
    /// A file on a mounted volume.
    File(PathBuf),
}

pub(crate) struct HttpLocation<'a> {
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub provider: Option<&'a dyn TokenProvider>,
}

impl Location {
    pub(crate) fn http(&self) -> Option<HttpLocation<'_>> {
        match self {
            Self::Url { url, headers, .. } => Some(HttpLocation { url, headers, provider: None }),
            Self::Authenticated { url, provider } => Some(HttpLocation { url, headers: &[], provider: Some(provider.as_ref()) }),
            Self::File(_) => None,
        }
    }
}

impl std::fmt::Debug for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(path) => f.debug_tuple("File").field(path).finish(),
            Self::Url { url, .. } => f.debug_struct("Url").field("url", &super::net::shown(url)).finish_non_exhaustive(),
            Self::Authenticated { url, .. } => f.debug_struct("Authenticated").field("url", url).finish_non_exhaustive(),
        }
    }
}
