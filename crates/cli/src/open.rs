//! Open the current project in a browser and print its local URL. A `--json` caller is a program: it gets the link, and no browser starts.
use std::{
    net::{Ipv4Addr, TcpListener},
    process::ExitCode,
};

use api::{
    calcifer::{SessionCode, project::Address},
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
#[command(after_help = "With --json it prints the link and opens no browser.\n\nExamples:\n  cd my-project && cozy open\n  cozy open --port 8818")]
pub(crate) struct Args {
    /// Use this loopback port for Calcifer, and keep it for later commands.
    #[arg(long, value_name = "PORT", value_parser = clap::value_parser!(u16).range(1..))]
    port: Option<u16>,
}

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    let (calcifer, project) = match args.port {
        Some(port) => select_port(ctx, port).await?,
        None => {
            let project = project::identity(ctx).await?;
            (ctx.calcifer().await?, project)
        }
    };
    // A `--json` caller is a program: it gets a one-use link, and no browser starts.
    let opened = if ctx.json {
        json!({ "url": sign_in_link(&calcifer, &project).await?, "private": false })
    } else { match calcifer.post::<_, Opened>("/v1/browser/open", &json!({ "project": project.id }), None).await {
        // The printed URL is reference-only; authority was injected through the owned native pipe.
        Ok(opened) => json!({ "url": opened.url, "private": true }),
        // No display, no Chrome, or its launch failed: any browser may sign in with a one-use link instead.
        Err(error) if error.code == "private_browser_unavailable" => {
            let reason = error.cause.clone().unwrap_or(error.code);
            let url = sign_in_link(&calcifer, &project).await?;
            let name = match crate::project::current(ctx).await {
                Ok(state) => state.path.file_name().map(|name| name.to_string_lossy().into_owned()),
                Err(_) => None,
            }.unwrap_or_else(|| project.code.clone());
            output::warn(&format!("Cozy's private browser couldn't start ({reason}): {name} opens in any browser with this one-use sign-in link, which isn't private"));
            json!({ "url": url, "private": false, "reason": reason })
        }
        Err(error) => return Err(error),
    } };
    output::document(ctx, &opened);
    Ok(ExitCode::SUCCESS)
}

#[derive(serde::Deserialize)]
struct Opened {
    url: String,
}

/// The project's page with a one-use sign-in code in its fragment, which any browser redeems once.
async fn sign_in_link(calcifer: &Client, project: &Address) -> Result<String, Error> {
    let code: SessionCode = calcifer.post("/v1/session-codes", &json!({}), None).await?;
    let mut link = url::Url::parse(&code.url).map_err(|error| Error::new(Kind::Internal, "session_url", error.to_string()))?;
    link.path_segments_mut().map_err(|()| Error::new(Kind::Internal, "session_url", "the sign-in URL has no path"))?.clear().extend(["projects", &project.id]);
    link.set_fragment(Some(&format!("code={}", code.code)));
    Ok(link.into())
}

fn reserve_port(port: u16) -> Result<TcpListener, Error> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|error| {
        Error::new(Kind::Unavailable, "calcifer_port", format!("Calcifer cannot listen on 127.0.0.1:{port}: {error}"))
            .remedy("Choose an unused port")
            .next("cozy open --port <port>")
    })
}

async fn select_port(ctx: &Context, port: u16) -> Result<(Client, Address), Error> {
    let port_error = |error: platform::Error| Error::new(Kind::Unavailable, "calcifer_port", error.to_string());
    ctx.home.open().map_err(port_error)?;
    let _held = LockFile::new(ctx.home.root().join("calcifer-port.lock"))
        .lock(LockKind::Exclusive).map_err(port_error)?;
    let unconnected = Client::unconnected(ctx)?;
    let running = match unconnected.lock_state() {
        Lock::Free { .. } => None,
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
        project::identity(ctx).await
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
