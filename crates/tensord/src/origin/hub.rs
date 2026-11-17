//! A model at a Hub: TensorFS's distribution `Endpoint` at the Hub's origin, presenting the
//! signer's newest run capability when the request carried one (the Hub reads the checkpoints it names
//! to it), else anonymous (released content). The Hub resolves a
//! release and lane, or the machine's hardware, to one checkpoint. Every Hub call runs on the fetch
//! runtime.
use std::sync::Arc;

use tensorfs::{
    err::{refuse, Code, Result},
    fetch::{Endpoint, Hardware, Resolved, TokenProvider},
    format::ObjectRef,
};

use super::{Key, Machine, Remote};
use crate::{hub, stop::Stop};

/// `source`'s Hub as an endpoint over the machine's client for its CA.
fn endpoint(machine: &Machine, source: &hub::Source) -> Result<(Endpoint, Key)> {
    let trust = source.ca_der.as_deref().map(|der| Arc::new(hub::pem(der).into_bytes()));
    let client = machine.client(trust.as_ref())?;
    let endpoint = match &source.capability {
        Some(capability) => Endpoint::new(&client, &source.origin, Some(capability.clone() as Arc<dyn TokenProvider>))?,
        None => Endpoint::new(&client, &source.origin, None)?.anonymous(),
    };
    Ok((endpoint, trust))
}

/// Where the objects of `source`'s Hub are read: those of `commit` (a tree, and the components read; none: all) where
/// one grant of the Hub's says (xs-210). Nothing is asked of the Hub until a fetch needs it.
pub(crate) fn remote(machine: &Machine, source: &hub::Source, commit: Option<(&ObjectRef, &[String])>) -> Result<Remote> {
    let (mut endpoint, trust) = endpoint(machine, source)?;
    if let Some((tree, components)) = commit {
        endpoint = endpoint.granting(tree, components);
    }
    Ok(Remote { origin: Arc::new(endpoint), trust })
}

/// `model`'s checkpoint at `source`'s Hub: of `release` (empty: the newest) in `lane` (empty:
/// the one `hardware` runs best).
pub(crate) fn resolve(machine: &Machine, source: &hub::Source, model: &str, release: &str, lane: &str, hardware: &Hardware) -> Result<Resolved> {
    let (endpoint, _) = endpoint(machine, source)?;
    let (model, release, lane, hardware) = (model.to_string(), release.to_string(), lane.to_string(), hardware.clone());
    machine.block(&Stop::default(), async move {
        let some = |text: &str| (!text.is_empty()).then(|| text.to_owned());
        endpoint.resolve(&model, some(&release).as_deref(), some(&lane).as_deref(), &hardware).await
    })
}

/// The object `sha256` at `source`'s Hub, with its length, as the request may read it.
pub(crate) fn object(machine: &Machine, source: &hub::Source, sha256: &str) -> Result<ObjectRef> {
    let (endpoint, _) = endpoint(machine, source)?;
    let sha256 = sha256.to_string();
    machine.block(&Stop::default(), async move {
        match endpoint.object(&sha256).await? {
            Some(object) => Ok(object),
            None => refuse(Code::REF_NOT_FOUND, format!("sha256:{sha256} is not readable at {}", endpoint.url())),
        }
    })
}
