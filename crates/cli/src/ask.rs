//! Asks Calcifer turns into runs (model downloads and uploads, package installs, quantizations):
//! one POST under a fresh key, a `new` run quoted from the same body first and agreed to.
use protocol::{
    calcifer::{Agreed, On, RunQuote},
    error::{Error, Kind},
};
use serde::{Serialize, de::DeserializeOwned};

use crate::{Context, run::mint_key};

/// `ask` at `route`; when it rents (`on` is `new`), quoted at `quotes` and agreed to first, the
/// agreed price set by `agreed`.
pub(crate) async fn start<A: Serialize, T: DeserializeOwned>(
    ctx: &Context,
    route: &str,
    quotes: &str,
    on: &On,
    ask: &mut A,
    agreed: impl FnOnce(&mut A, Agreed),
) -> Result<T, Error> {
    let calcifer = ctx.calcifer().await?;
    if matches!(on, On::New(_)) {
        let quoted: RunQuote = calcifer.post(quotes, ask, None).await?;
        if !crate::machine::agree(ctx, &quoted)? {
            return Err(Error::new(Kind::Usage, "quote_declined", "nothing was rented and the run was not sent"));
        }
        agreed(ask, quoted.quote.agreed());
    }
    calcifer.post(route, ask, Some(&mint_key())).await
}

/// `raw`, absolute when it names a local path (`./x`, `/x`): Calcifer reads it from its own
/// working directory.
pub(crate) fn absolute(raw: &str) -> Result<String, Error> {
    let path = std::path::Path::new(raw);
    if !raw.starts_with('.') && !path.is_absolute() {
        return Ok(raw.to_owned());
    }
    let absolute = std::path::absolute(path)
        .map_err(|e| Error::new(Kind::Usage, "path_unreadable", format!("{raw}: {e}")))?;
    Ok(absolute.to_string_lossy().into_owned())
}
