//! Open the current project in a browser and print its local URL. A `--json` caller is a program: it gets the link, and no browser starts.
use std::{
    io::{self, Write},
    net::{Ipv4Addr, TcpListener},
    process::{Command, ExitCode, Stdio},
};

use api::{
    calcifer::{SessionCode, project::State},
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
            let project = current_or_unlock(ctx).await?;
            (ctx.calcifer().await?, project)
        }
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
    let name = project.path.file_name().map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| project.code.clone());
    if !ctx.json && let Err(error) = browse(ctx, link.as_str()) {
        output::warn(&format!("could not open the browser: {error}; use the URL below"));
    }
    output::document(ctx, &json!({ "project": name, "url": link.as_str() }));
    Ok(ExitCode::SUCCESS)
}

/// One explicit Open may prompt for an unlock. Pipes and JSON never acquire credentials implicitly.
async fn current_or_unlock(ctx: &Context) -> Result<State, Error> {
    match project::current(ctx).await {
        Err(error) if error.code == "project_locked" && output::can_ask(ctx) => {
            project::vault::unlock_for_open(ctx).await?;
            project::current(ctx).await
        }
        answer => answer,
    }
}

/// Keep the link's one-use code out of the browser process's command line.
fn browse(ctx: &Context, url: &str) -> io::Result<()> {
    let cache = ctx.home.root().join("cache");
    std::fs::create_dir_all(&cache)?;
    let mut redirect = tempfile::Builder::new().prefix("open-").suffix(".html").tempfile_in(cache)?;
    let escaped = url.replace('&', "&amp;").replace('"', "&quot;");
    write!(redirect, "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"0; url={escaped}\"><title>Cozy</title><a href=\"{escaped}\">Open Cozy</a>")?;
    let path = redirect.into_temp_path().keep().map_err(|error| error.error)?;
    let (program, arguments): (&str, &[&str]) = match std::env::consts::OS {
        "macos" => ("open", &[]),
        "windows" => ("cmd", &["/C", "start", ""]),
        _ => ("xdg-open", &[]),
    };
    let result = Command::new(program).args(arguments).arg(&path)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result.map(drop)
}

fn reserve_port(port: u16) -> Result<TcpListener, Error> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|error| {
        Error::new(Kind::Unavailable, "calcifer_port", format!("Calcifer cannot listen on 127.0.0.1:{port}: {error}"))
            .remedy("Choose an unused port")
            .next("cozy open --port <port>")
    })
}

async fn select_port(ctx: &Context, port: u16) -> Result<(Client, State), Error> {
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
        current_or_unlock(ctx).await
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
