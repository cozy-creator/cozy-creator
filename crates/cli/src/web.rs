//! `cozy web` and `cozy project open`: this computer's Calcifer in a browser, signed in by a single-use code. The code
//! goes in a 0600 redirect file and the browser opens the file, so it's in no process's arguments.
use std::process::{Command, ExitCode, Stdio};

use protocol::{calcifer::SessionCode, error::Error};
use serde_json::json;

use crate::{Context, output};

/// Open the web UI in your browser.
#[derive(clap::Args, Debug)]
#[command(after_help = "Examples:\n  cozy web\n  cozy web --no-open   prints a link instead: over SSH, say")]
pub struct Args {
    /// Print a link instead of opening it. It works once, for 60 s.
    #[arg(long)]
    no_open: bool,
}

pub async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    open(ctx, None, args.no_open).await
}

/// The page, at `to`, one of its routes. A build without its page is refused here, saying how to build it.
pub(crate) async fn open(ctx: &Context, to: Option<String>, no_open: bool) -> Result<ExitCode, Error> {
    let calcifer = ctx.calcifer().await?;
    calcifer.text("/").await?;
    let code: SessionCode = calcifer.post("/v1/session-codes", &json!({}), None).await?;
    let link = match to {
        Some(to) => format!("{}&to={to}", code.url),
        None => code.url,
    };
    let opened = !no_open
        && match browse(ctx, &link) {
            Ok(()) => true,
            Err(why) => {
                output::warn(&format!("couldn't open your browser ({why}), so here's a link"));
                false
            }
        };
    if opened {
        if ctx.json {
            output::json(&json!({ "opened": true }));
        }
        output::say(ctx, "opened in your browser. If nothing came up, or it can't read the file (a snap or Flatpak browser can't), run this again with --no-open for a link");
        return Ok(ExitCode::SUCCESS);
    }
    if ctx.json {
        output::json(&json!({ "url": link, "expires_ms": code.expires_ms }));
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
    let file = ctx.home.calcifer().join("open.html");
    let link = link.replace('&', "&amp;");
    let page = format!(r#"<!doctype html><meta charset="utf-8"><meta http-equiv="refresh" content="0; url={link}"><title>cozy</title><a href="{link}">Open cozy</a>"#);
    platform::durable::replace(&file, page.as_bytes(), 0o600).map_err(|e| e.to_string())?;
    let (program, first): (&str, &[&str]) = match std::env::consts::OS {
        "macos" => ("open", &[]),
        "windows" => ("cmd", &["/C", "start", ""]),
        _ => ("xdg-open", &[]),
    };
    let opener = Command::new(program).args(first).arg(&file).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    opener.map(drop).map_err(|e| format!("{program}: {e}"))
}
