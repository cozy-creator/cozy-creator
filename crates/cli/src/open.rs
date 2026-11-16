//! Open the current project in the default browser and print its local URL. A `--json` caller is a program: it gets the link, and no
//! browser starts.
use std::{
    ffi::OsString,
    net::{Ipv4Addr, TcpListener},
    path::Path,
    process::ExitCode,
    time::{Duration, Instant},
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
    let url = page_url(&calcifer, &project)?;
    // A `--json` caller is a program: it gets the page's address, and no browser starts.
    if !ctx.json && let Err(why) = browse(&url).await {
        output::warn(&format!("no browser could be started ({why}): open this address in any browser"));
    }
    output::document(ctx, &json!({ "url": url }));
    Ok(ExitCode::SUCCESS)
}

/// `url` in the default browser, as any local app opens one: `$BROWSER` when set (its commands in turn, `:`-separated, `;` on Windows;
/// `%s` is the URL, else it goes last), else the system's opener. The browser is its own, and outlives `cozy`. Why none started, if none did.
async fn browse(url: &str) -> Result<(), String> {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let commands: Vec<Vec<String>> = match std::env::var("BROWSER") {
        Ok(configured) if !configured.trim().is_empty() => configured.split(separator).filter_map(|command| {
            let mut words: Vec<String> = command.split_whitespace().map(|word| word.replace("%s", url)).collect();
            if words.is_empty() { return None; }
            if !command.contains("%s") { words.push(url.to_owned()); }
            Some(words)
        }).collect(),
        _ => vec![system_opener(url).ok_or_else(|| "no display".to_owned())?],
    };
    let mut why = Vec::new();
    for words in commands {
        match start(&words).await {
            Ok(()) => return Ok(()),
            Err(failed) => why.push(failed),
        }
    }
    Err(why.join("; "))
}

/// The system's way to open a link: `open` (macOS), `start` (Windows), `xdg-open` (others), which without a display would start a text
/// browser in this terminal, so none then.
fn system_opener(url: &str) -> Option<Vec<String>> {
    let words: &[&str] = if cfg!(target_os = "macos") {
        &["open"]
    } else if cfg!(windows) {
        &["cmd", "/C", "start", ""]
    } else if std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        &["xdg-open"]
    } else {
        return None;
    };
    Some(words.iter().map(|word| (*word).to_owned()).chain([url.to_owned()]).collect())
}

/// `words` started on their own, given a moment to fail: one that exits well, or is still going, opened the page.
async fn start(words: &[String]) -> Result<(), String> {
    let program = &words[0];
    let args: Vec<OsString> = words[1..].iter().map(OsString::from).collect();
    let mut started = platform::process::detach(Path::new(program), &args, None).map_err(|e| format!("{program}: {e}"))?;
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        match started.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{program} {}", platform::process::ended(&status).0)),
            Ok(None) => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(e) => return Err(format!("{program}: {e}")),
        }
    }
    Ok(())
}

/// The project's page on this Calcifer: any browser of this account opens it as Calcifer's own page.
fn page_url(calcifer: &Client, project: &Address) -> Result<String, Error> {
    let holder = calcifer.holder().ok_or_else(|| Error::new(Kind::Unavailable, "calcifer_unavailable", "Calcifer isn't running").next("cozy open"))?;
    Ok(format!("http://{}/projects/{}", holder.address, project.id))
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
