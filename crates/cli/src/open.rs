//! Open the current project in a private window of the user's own browser and print its local URL; outside a project, the page's start,
//! where any project is a click away (cl-1179). A `--json` caller is a program: it gets the link, and no browser starts.
mod window;

use std::{
    net::{Ipv4Addr, TcpListener},
    process::ExitCode,
};

use api::{
    calcifer::project::Address,
    config,
    error::{Error, Kind},
};
use platform::lock::{Kind as LockKind, LockFile};
use serde_json::json;

use crate::{
    Context,
    calcifer_client::{Client, Lock},
    output, project,
};

#[derive(clap::Args, Debug)]
#[command(after_help = "It opens a private window of your browser ($BROWSER, else the default). Outside a project folder it opens Cozy Creator, where you can pick one. With --json it prints the link and opens no browser.\n\nExamples:\n  cd my-project && cozy open\n  cozy open --port 8818")]
pub(crate) struct Args {
    /// Use this loopback port for Calcifer, and keep it for later commands.
    #[arg(long, value_name = "PORT", value_parser = clap::value_parser!(u16).range(1..))]
    port: Option<u16>,
}

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    let (calcifer, project) = match args.port {
        Some(port) => select_port(ctx, port).await?,
        None => {
            let project = here(ctx).await?;
            (ctx.calcifer().await?, project)
        }
    };
    let url = page_url(&calcifer, project.as_ref())?;
    // A `--json` caller is a program: it gets the page's address, and no browser starts.
    if ctx.json {
        let mut document = json!({ "url": url });
        if project.is_none() {
            document["note"] = json!({ "code": "not_in_a_project", "message": OUTSIDE, "next": ["cozy new"] });
        }
        output::document(ctx, &document);
        return Ok(ExitCode::SUCCESS);
    }
    if project.is_none() {
        output::say(ctx, OUTSIDE);
    }
    match window::open(&url).await {
        Ok(browser) => output::document(ctx, &json!({ "url": url, "browser": browser })),
        Err(why) => {
            // Never a normal window: the point is that nothing of the project lands in the browser's history or cache.
            output::warn(&format!("no browser with a private window could be started ({why}): open this address in one"));
            output::document(ctx, &json!({ "url": url }));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Paul, 11-17: "we should tell the user 'you are not in a project folder, please navigate to one or create one using cozy new' but we
/// should also open the webui anyway, because they can select between different projects in it anyway" (cl-1179).
const OUTSIDE: &str = "You're not in a project folder, so this opens Cozy Creator, where you can pick one. To open a project directly, run cozy open in its folder, or create one with cozy new.";

/// The project here, None outside any: then the page opens at its start.
async fn here(ctx: &Context) -> Result<Option<Address>, Error> {
    match project::identity(ctx).await {
        Ok(project) => Ok(Some(project)),
        Err(error) if error.code == "not_in_a_project" => Ok(None),
        Err(error) => Err(error),
    }
}

/// The project's page on this Calcifer, `/p/<code>`: a project's only address (cl-1171); with none, the page's start, `/`, its empty
/// canvas under every project's tab. Any browser of this account opens it as Calcifer's own page.
fn page_url(calcifer: &Client, project: Option<&Address>) -> Result<String, Error> {
    let holder = calcifer.holder().ok_or_else(|| Error::new(Kind::Unavailable, "calcifer_unavailable", "Calcifer isn't running").next("cozy open"))?;
    Ok(match project {
        Some(project) => format!("http://{}/p/{}", holder.address, project.code),
        None => format!("http://{}/", holder.address),
    })
}

fn reserve_port(port: u16) -> Result<TcpListener, Error> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|error| {
        Error::new(Kind::Unavailable, "calcifer_port", format!("Calcifer cannot listen on 127.0.0.1:{port}: {error}"))
            .remedy("Choose an unused port")
            .next("cozy open --port <port>")
    })
}

async fn select_port(ctx: &Context, port: u16) -> Result<(Client, Option<Address>), Error> {
    let port_error = |error: platform::Error| Error::new(Kind::Unavailable, "calcifer_port", error.to_string());
    ctx.home.open().map_err(port_error)?;
    let _held = LockFile::new(ctx.home.root().join("calcifer-port.lock"))
        .lock(LockKind::Exclusive).map_err(port_error)?;
    let unconnected = Client::unconnected(ctx)?;
    let running = match unconnected.lock_state() {
        Lock::Free => None,
        _ => Client::running(ctx).await?,
    };
    let was_running = running.is_some();
    let calcifer = running.unwrap_or(unconnected);
    let switching = !calcifer.holder().is_some_and(|holder| holder.address.port() == port);
    let reservation = switching.then(|| reserve_port(port)).transpose()?;
    let previous = config::set_calcifer_port(&ctx.home, Some(port))?;
    let selected = async {
        if switching && was_running {
            calcifer.stop().await?;
        }
        drop(reservation);
        let holder = calcifer.ready().await?;
        if holder.address.port() != port {
            return Err(Error::new(
                Kind::Conflict, "calcifer_port", format!("Calcifer did not start on the requested port {port}"),
            ));
        }
        here(ctx).await
    }.await;
    match selected {
        Ok(project) => Ok((calcifer, project)),
        Err(error) => {
            config::set_calcifer_port(&ctx.home, previous)?;
            if switching && calcifer.holder().is_some_and(|holder| holder.address.port() == port) {
                calcifer.stop().await?;
            }
            if switching && was_running {
                calcifer.ready().await?;
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Open {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn port_argument_accepts_only_nonzero_u16_values() {
        for port in ["1", "8818", "65535"] {
            assert_eq!(Open::try_parse_from(["open", "--port", port]).unwrap().args.port, port.parse().ok());
        }
        for port in ["0", "65536", "-1", "localhost:8818"] {
            assert!(Open::try_parse_from(["open", "--port", port]).is_err());
        }
        assert_eq!(Open::try_parse_from(["open"]).unwrap().args.port, None);
    }

    #[test]
    fn occupied_port_is_refused_before_config_or_restart() {
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let error = reserve_port(port).unwrap_err();
        assert_eq!(error.code, "calcifer_port");
        assert!(error.message.contains(&format!("127.0.0.1:{port}")));
    }
}
