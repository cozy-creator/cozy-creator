//! A model at a Hub: TensorFS's distribution `Endpoint` at the Hub's origin, presenting the
//! signer's newest run capability when the request carried one (the Hub reads the commits it names
//! to it), else anonymous (released content). The Hub lists a release's lanes; the machine picks
//! one (`tensorfs::lanes`). Every Hub call runs on the fetch runtime.
use std::sync::Arc;

use tensorfs::{
    err::{refuse, Code, Result},
    fetch::{Endpoint, Released, TokenProvider},
    format::ObjectRef,
};

use super::{Key, Machine, Remote};
use crate::{hub, stop::Stop};

/// `source`'s Hub as an endpoint over the machine's client for it (its CA, whether it is on
/// loopback).
fn endpoint(machine: &Machine, source: &hub::Source) -> Result<(Endpoint, Key)> {
    let trust = source.ca_der.as_deref().map(|der| Arc::new(hub::pem(der).into_bytes()));
    let local = hub::origin_key(&source.origin).is_some_and(|key| loopback(&key));
    let client = machine.client(trust.as_ref(), local)?;
    let endpoint = match &source.capability {
        Some(capability) => Endpoint::new(&client, &source.origin, Some(capability.clone() as Arc<dyn TokenProvider>))?,
        None => Endpoint::new(&client, &source.origin, None)?.anonymous(),
    };
    Ok((endpoint, (trust, local)))
}

/// Where the objects of `source`'s Hub are read. Nothing is asked of the Hub until a fetch
/// needs it.
pub fn remote(machine: &Machine, source: &hub::Source) -> Result<Remote> {
    let (endpoint, (trust, local)) = endpoint(machine, source)?;
    Ok(Remote { origin: Arc::new(endpoint), trust, local })
}

/// `repository`'s `release` (empty: the newest) at `source`'s Hub, with every lane of it.
pub fn release(machine: &Machine, source: &hub::Source, repository: &str, release: &str) -> Result<Released> {
    let (endpoint, _) = endpoint(machine, source)?;
    let (repository, release) = (repository.to_string(), release.to_string());
    machine.block(&Stop::default(), async move {
        endpoint.release(&repository, (!release.is_empty()).then_some(release.as_str())).await
    })
}

/// The object `sha256` at `source`'s Hub, with its length, as the request may read it.
pub fn object(machine: &Machine, source: &hub::Source, sha256: &str) -> Result<ObjectRef> {
    let (endpoint, _) = endpoint(machine, source)?;
    let sha256 = sha256.to_string();
    machine.block(&Stop::default(), async move {
        match endpoint.object(&sha256).await? {
            Some(object) => Ok(object),
            None => refuse(Code::REF_NOT_FOUND, format!("sha256:{sha256} is not readable at {}", endpoint.url())),
        }
    })
}

fn loopback(key: &str) -> bool {
    let host = key.split_once("://").and_then(|(_, rest)| rest.rsplit_once(':')).map(|(host, _)| host);
    matches!(host, Some("localhost" | "127.0.0.1" | "[::1]"))
}
