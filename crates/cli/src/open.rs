//! Print the authenticated local browser URL for the current project.
use std::{
    net::{Ipv4Addr, TcpListener},
    process::ExitCode,
};

use api::{
    calcifer::SessionCode,
    config,
    error::{Error, Kind},
};
use platform::lock::{Kind as LockKind, LockFile};
use serde_json::json;

use crate::{Context, calcifer_client::Client, output, project};

#[derive(clap::Args, Debug)]
#[command(after_help = "Examples:\n  cd my-project && cozy open\n  cozy open --port 8818")]
pub(crate) struct Args {
    /// Use this loopback port for Calcifer, and keep it for later commands.
    #[arg(long, value_name = "PORT", value_parser = clap::value_parser!(u16).range(1..))]
    port: Option<u16>,
}

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    let project = project::current(ctx).await?;
    let calcifer = match args.port {
        Some(port) => select_port(ctx, port).await?,
        None => ctx.calcifer().await?,
    };
    calcifer.text("/").await?;
    let code: SessionCode = calcifer.post("/v1/session-codes", &json!({}), None).await?;
    let mut link = url::Url::parse(&code.url)
        .map_err(|error| Error::new(Kind::Internal, "session_url", error.to_string()))?;
    let path = if project.code.is_empty() {
        format!("/projects/{}", project.id)
    } else {
        format!("/p/{}", project.code)
    };
    link.set_path(&path);
    link.set_fragment(Some(&format!("code={}", code.code)));
    output::document(ctx, &json!({ "url": link.as_str(), "expires_ms": code.expires_ms }));
    Ok(ExitCode::SUCCESS)
}

fn reserve_port(port: u16) -> Result<TcpListener, Error> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|error| {
        Error::new(Kind::Unavailable, "calcifer_port", format!("Calcifer cannot listen on 127.0.0.1:{port}: {error}"))
            .remedy("choose a free loopback port with cozy open --port <port>")
    })
}

async fn select_port(ctx: &Context, port: u16) -> Result<Client, Error> {
    let _held = LockFile::new(ctx.home.root().join("calcifer-port.lock"))
        .lock(LockKind::Exclusive).map_err(|error| Error::new(Kind::Unavailable, "calcifer_port", error.to_string()))?;
    let calcifer = ctx.calcifer().await?;
    let running = calcifer.holder().expect("connected Calcifer has a holder");
    let reservation = if running.address.port() == port { None } else { Some(reserve_port(port)?) };
    let previous = config::set_calcifer_port(&ctx.home, Some(port))?;
    if reservation.is_none() {
        return Ok(calcifer);
    }
    let stopped = calcifer.stop().await;
    drop(reservation);
    let started = match stopped {
        Ok(_) => calcifer.ready().await,
        Err(error) => Err(error),
    };
    match started {
        Ok(holder) if holder.address.port() == port => Ok(calcifer),
        started => {
            let error = started.err().unwrap_or_else(|| Error::new(
                Kind::Conflict, "calcifer_port", format!("Calcifer did not start on the requested port {port}"),
            ));
            config::set_calcifer_port(&ctx.home, previous)?;
            ctx.calcifer().await?;
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
