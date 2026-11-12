//! Open the current project in a browser and print its local URL.
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
#[command(after_help = "Examples:\n  cd my-project && cozy open\n  cozy open --port 8818")]
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
    let opened: serde_json::Value = calcifer.post("/v1/browser/open", &json!({ "project": project.id }), None).await?;
    // The printed URL is reference-only; authority was injected through the owned native pipe.
    output::document(ctx, &opened);
    Ok(ExitCode::SUCCESS)
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
