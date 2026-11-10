//! `cozy open`: the current directory's project in a browser, signed in by a single-use code. The code goes in a
//! 0600 redirect file and the browser opens the file, so it's in no process's arguments. Only with no browser to open is the link
//! printed: it works once, for 60 s.
use std::process::{Command, ExitCode, Stdio};

use api::{calcifer::SessionCode, error::Error};
use serde_json::json;

use crate::{Context, output, project};

/// Open the current directory's project in your browser.
#[derive(clap::Args, Debug)]
#[command(after_help = "Examples:\n  cd garden && cozy open\n  cozy open --no-open   only prints the link: for an agent, or over SSH")]
pub(crate) struct Args {
    /// Only print the link, which works once, for 60 s.
    #[arg(long)]
    no_open: bool,
}

/// Open the existing project containing the current directory.
async fn route(ctx: &Context) -> Result<String, Error> {
    let state = project::current(ctx).await?;
    Ok(format!("/projects/{}", state.id))
}

/// Whether a browser here would be in front of anyone: none over SSH, and on Linux none without a display.
fn in_front_of_someone() -> bool {
    let has = |name: &str| std::env::var_os(name).is_some();
    !has("SSH_CONNECTION") && (std::env::consts::OS != "linux" || has("DISPLAY") || has("WAYLAND_DISPLAY"))
}

/// A build without its page is refused here, saying how to build it.
pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    let to = route(ctx).await?;
    let calcifer = ctx.calcifer().await?;
    calcifer.text("/").await?;
    let code: SessionCode = calcifer.post("/v1/session-codes", &json!({}), None).await?;
    let link = format!("{}&to={to}", code.url);
    let opened = !args.no_open
        && in_front_of_someone()
        && match browse(ctx, &link) {
            Ok(()) => true,
            Err(why) => {
                output::warn(&format!("couldn't open your browser: {why}"));
                false
            }
        };
    if ctx.json {
        output::json(&json!({ "url": link, "expires_ms": code.expires_ms, "opened": opened }));
        return Ok(ExitCode::SUCCESS);
    }
    if opened {
        output::say(ctx, "opened in your browser; if it didn't come up: cozy open --no-open");
        return Ok(ExitCode::SUCCESS);
    }
    anstream::println!("{link}");
    output::say(ctx, "it works once, for 60 s");
    if let Some(holder) = calcifer.holder() {
        let port = holder.address.port();
        output::say(ctx, &format!("from another computer, forward its port first: ssh -L {port}:127.0.0.1:{port} <this computer>"));
    }
    Ok(ExitCode::SUCCESS)
}

/// Writes the redirect and asks this computer's browser to open it, without waiting. Its code is spent at once or dead
/// in 60 s, so one file, replaced each time, is all that ever lies there.
fn browse(ctx: &Context, link: &str) -> Result<(), String> {
    let file = ctx.home.root().join("cache").join("open.html");
    let link = link.replace('&', "&amp;");
    let page = format!(r#"<!doctype html><meta charset="utf-8"><meta http-equiv="refresh" content="0; url={link}"><title>cozy</title><a href="{link}">Open cozy</a>"#);
    std::fs::create_dir_all(ctx.home.root().join("cache")).map_err(|e| e.to_string())?;
    platform::durable::replace(&file, page.as_bytes(), 0o600).map_err(|e| e.to_string())?;
    let (program, first): (&str, &[&str]) = match std::env::consts::OS {
        "macos" => ("open", &[]),
        "windows" => ("cmd", &["/C", "start", ""]),
        _ => ("xdg-open", &[]),
    };
    let opener = Command::new(program).args(first).arg(&file).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    opener.map(drop).map_err(|e| format!("{program}: {e}"))
}
