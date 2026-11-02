//! `cozy project`: the CLI's side of Calcifer's projects (xs-136, cl-716). Calcifer finds the
//! project, makes its takes and versions and keeps its assets; these verbs ask it and print, and
//! `render` follows each run as `cozy run` does. Ctrl-C stops following, never the runs.
use std::{path::PathBuf, process::ExitCode};

use protocol::{
    calcifer::{On, local_usable},
    error::{Error, Kind},
    project::{Approve, Approved, Event, Open, Opened, Projects, Rename, Renamed, Runs, State},
};

use crate::{
    Context, output,
    run::{follow, mint_key, settled},
};

const EXAMPLES: &str = "Examples:
  cozy project new garden
  cd garden && cozy project render all --on kiyui
  cozy project render fight -n 3 --on kiyui    three takes of fight
  cozy project approve fight 2
  cozy project rename theo mara                after renaming it in project.yaml

project.yaml's comments and the README explain the rest.";

const RENDER_EXAMPLES: &str = "Examples:
  cozy project render all --on kiyui         everything missing or out of date, then the edit
  cozy project render all --on kiyui,jitae   spread over two machines
  cozy project render fight -n 3 --on kiyui  three takes of fight
  cozy project render fight --approve --on kiyui

Ctrl-C stops following, never the runs.";

/// A video project: a folder of nodes (characters, props, environments and their camera setups,
/// world states, shots) whose takes and versions `cozy project render` makes.
#[derive(clap::Args, Debug)]
#[command(after_help = EXAMPLES)]
pub struct Args {
    /// The project's folder, if not the one you are in.
    #[arg(short = 'p', value_name = "PATH", global = true)]
    path: Option<PathBuf>,
    #[command(subcommand)]
    verb: Verb,
}

#[derive(clap::Subcommand, Debug)]
enum Verb {
    /// Make a project folder.
    #[command(after_help = "Example:\n  cozy project new garden && $EDITOR garden/project.yaml")]
    New {
        /// The folder to make it in.
        path: PathBuf,
    },
    /// List your projects.
    #[command(after_help = "Example:\n  cozy project ls")]
    Ls,
    Render(Render),
    /// Approve a take or version.
    #[command(after_help = "Example:\n  cozy project approve fight 2")]
    Approve {
        /// The node: `theo`, `dock.b` (a setup), `fight`, `fight.upscale`, …
        name: String,
        /// Its take or version, 1 first.
        version: u32,
    },
    /// What is missing or out of date, and each node's file.
    #[command(after_help = "Example:\n  cozy project status")]
    Status,
    /// Keep a node's versions after renaming it in project.yaml.
    #[command(after_help = "Example:\n  cozy project rename theo mara   (status suggests it after a rename)")]
    Rename {
        /// Its name in project.lock.
        from: String,
        /// Its name in project.yaml now: a node of the same kind with no versions.
        to: String,
    },
}

/// Make new takes and versions; `all` brings everything up to date.
#[derive(clap::Args, Debug)]
#[command(after_help = RENDER_EXAMPLES)]
struct Render {
    /// The nodes (`theo`, `dock.b`, `fight`, `fight.upscale`, …) to make new takes or versions of,
    /// or `all`: everything missing or out of date, in order, each new one approved, then the edit.
    #[arg(required = true, value_name = "NAME")]
    names: Vec<String>,
    /// The machines the renders go to: `local`, machine names, or `any`, comma-separated.
    #[arg(long = "on", value_name = "MACHINE", value_delimiter = ',', default_value = "local")]
    on: Vec<String>,
    /// How many new takes or versions of each.
    #[arg(short = 'n', value_name = "N", default_value_t = 1)]
    n: u32,
    /// Approve the new take or version when it lands (only with -n 1).
    #[arg(long)]
    approve: bool,
    /// With `all`: no upscales; cut only the native preview (`preview.mp4`).
    #[arg(long)]
    preview: bool,
}

pub async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.verb {
        Verb::New { path } => {
            let opened = open(ctx, Some(&path), true).await?;
            let folder = output::link(ctx, &opened.path);
            output::say(ctx, &format!("made {folder}: add entities, world states and shots to its project.yaml"));
            output::next(ctx, &[&format!("cd {} && cozy project render all --on <machine>", opened.path.display())]);
        }
        Verb::Ls => {
            let listed: Projects = ctx.calcifer().await?.get("/v1/projects").await?;
            if ctx.json {
                output::json(&listed);
                return Ok(ExitCode::SUCCESS);
            }
            let table = output::Table::new(ctx, ["NAME", "FOLDER", "USED", "STATE"]);
            let rows = listed.projects.iter().map(|known| {
                let state = match (known.missing, known.busy) {
                    (true, _) => "missing",
                    (false, true) => "rendering",
                    (false, false) => "-",
                };
                let used = format!("{} ago", output::age(known.used_at_ms));
                [known.name.clone(), known.path.display().to_string(), used, state.into()]
            });
            rows.fold(table, output::Table::row).print();
            if listed.older > 0 {
                output::say(ctx, &format!("{} more, used earlier (?before=<id>)", listed.older));
            }
        }
        Verb::Render(render) => return self::render(ctx, args.path.as_ref(), render).await,
        Verb::Approve { name, version } => {
            let opened = open(ctx, args.path.as_ref(), false).await?;
            let path = format!("/v1/projects/{}/approve", opened.id);
            let approved: Approved =
                ctx.calcifer().await?.post(&path, &Approve { name: name.clone(), version }, None).await?;
            approved.warnings.iter().for_each(|warning| output::warn(warning));
            if ctx.json {
                output::json(&approved);
            } else {
                let (name, noun, number) = (&approved.name, &approved.noun, approved.number);
                let path = output::link(ctx, &opened.path.join(&approved.path));
                output::say(ctx, &format!("{name}: {noun} {number} approved ({path})"));
                if !approved.stale.is_empty() {
                    output::say(ctx, &format!("out of date now: {}", approved.stale.join(", ")));
                }
            }
        }
        Verb::Status => {
            let opened = open(ctx, args.path.as_ref(), false).await?;
            let state: State = ctx.calcifer().await?.get(&format!("/v1/projects/{}", opened.id)).await?;
            state.warnings.iter().for_each(|warning| output::warn(warning));
            if ctx.json {
                output::json(&state);
                return Ok(ExitCode::SUCCESS);
            }
            let table = output::Table::new(ctx, ["NAME", "KIND", "MADE", "APPROVED", "MADE BY", "STATUS", "FILE"]);
            let rows = state.nodes.iter().map(|n| {
                let made = match n.versions {
                    1 => format!("1 {}", n.noun),
                    count => format!("{count} {}s", n.noun),
                };
                let approved = match (n.approved, n.assets.is_empty()) {
                    (Some(number), _) => number.to_string(),
                    (None, false) => "attached".into(),
                    (None, true) => "-".into(),
                };
                let made_by = n.made_by.clone().unwrap_or_else(|| "-".into());
                let file = n.path.clone().unwrap_or_else(|| "-".into());
                [n.name.clone(), n.kind.clone(), made, approved, made_by, n.status.clone(), file]
            });
            rows.fold(table, output::Table::row).print();
            summary(ctx, &state);
        }
        Verb::Rename { from, to } => {
            let opened = open(ctx, args.path.as_ref(), false).await?;
            let path = format!("/v1/projects/{}/rename", opened.id);
            let renamed: Renamed = ctx.calcifer().await?.post(&path, &Rename { from, to }, None).await?;
            match ctx.json {
                true => output::json(&renamed),
                false => output::say(ctx, &format!("{} is {} now: its {} kept", renamed.from, renamed.to, renamed.versions)),
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn render(ctx: &Context, at: Option<&PathBuf>, args: Render) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let calcifer = ctx.calcifer().await?;
    let mut events = calcifer.watch::<Event>(&format!("/v1/projects/{}/events", opened.id)).await?;
    let on: Vec<On> =
        args.on.into_iter().map(|on| local_usable(&on).map(|()| On::from(on))).collect::<Result<_, _>>()?;
    let runs = Runs {
        names: args.names,
        n: args.n,
        on,
        hub: ctx.hub().to_string(),
        preview: args.preview,
        approve: args.approve,
    };
    let path = format!("/v1/projects/{}/runs", opened.id);
    let _: Opened = calcifer.post(&path, &runs, Some(&mint_key())).await?;
    let status = format!("cozy project -p {} status", opened.path.display());
    while let Some(event) = events.next().await {
        match event {
            Event::Started { run, .. } => {
                if !settled(&follow(ctx, calcifer.run(&run).await?).await?) {
                    output::say(ctx, "the project's runs go on in Calcifer");
                    output::next(ctx, &[&status]);
                    return Ok(ExitCode::SUCCESS);
                }
            }
            Event::Landed { name, noun, number, path, .. } => {
                let path = output::link(ctx, &opened.path.join(path));
                output::say(ctx, &format!("{name}: {noun} {number} ({path})"));
            }
            Event::Said { message } => output::say(ctx, &message),
            Event::Warning { message } => output::warn(&message),
            Event::Cut { cut, frames, fps, retimed, took_s } => {
                let path = output::link(ctx, &opened.path.join(&cut));
                output::say(ctx, &format!("cut {path}: {}{} in {took_s:.1} s", length(frames, fps), retimes(&retimed, fps)));
            }
            Event::Ended { error: Some(error) } => return Err(error),
            Event::Ended { error: None } => return Ok(ExitCode::SUCCESS),
        }
    }
    Err(Error::new(Kind::Unavailable, "project_events_ended", "Calcifer stopped telling how the runs go").next(status))
}

/// Under the table: what is missing, what is out of date and why, and where the cuts are.
fn summary(ctx: &Context, state: &State) {
    if !state.missing.is_empty() {
        output::say(ctx, &format!("missing: {}", state.missing.join(", ")));
    }
    if !state.stale.is_empty() {
        let stale: Vec<String> = state.stale.iter().map(|s| format!("{} ({})", s.name, s.why)).collect();
        output::say(ctx, &format!("out of date: {}", stale.join(", ")));
    }
    for orphan in &state.orphans {
        let (n, name, kind) = (orphan.versions, &orphan.name, &orphan.kind);
        let said = format!("project.lock keeps {n} made of {name} (a {kind}), which project.yaml no longer names");
        output::say(ctx, &said);
        let to = orphan.renamed_to.as_deref().unwrap_or("<its new name>");
        output::next(ctx, &[&format!("cozy project rename {} {to}", orphan.name)]);
    }
    for cut in &state.cuts {
        let skips = match cut.skips.as_slice() {
            [] => String::new(),
            skips => format!("; it skips {} (nothing made yet)", skips.join(", ")),
        };
        let name = cut.cut.trim_end_matches(".mp4");
        let made = cut.frames.map(|frames| format!(", {}", length(frames, state.fps))).unwrap_or_default();
        let retimed = retimes(&cut.retimed, state.fps);
        match cut.cut_at_ms {
            Some(at_ms) => output::say(
                ctx,
                &format!("{name}: {}{made}, cut {} ago{retimed}{skips}", output::link(ctx, &cut.path), output::age(at_ms)),
            ),
            None => output::say(ctx, &format!("{name}: not cut yet ({})", cut.path.display())),
        }
    }
    let uncut = state.cuts.iter().any(|cut| cut.cut_at_ms.is_none());
    if !state.missing.is_empty() || !state.stale.is_empty() || uncut {
        output::next(ctx, &["cozy project render all --on <machine>"]);
    }
}

/// A cut's length: `271 frames (11.3 s at 24 fps)`.
fn length(frames: u64, fps: f64) -> String {
    format!("{frames} frames ({:.1} s at {fps} fps)", frames as f64 / fps)
}

/// `; it retimes b (25 fps) to 24 fps`, when it does.
fn retimes(retimed: &[String], fps: f64) -> String {
    match retimed {
        [] => String::new(),
        takes => format!("; it retimes {} to {fps} fps", takes.join(", ")),
    }
}

/// The project holding `at` (else the folder you are in) as Calcifer opens it, made there first
/// when `create`.
async fn open(ctx: &Context, at: Option<&PathBuf>, create: bool) -> Result<Opened, Error> {
    let cwd = std::env::current_dir()
        .map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let path = match at {
        Some(at) => protocol::target::spelled(&at.to_string_lossy(), &cwd, &ctx.user_home),
        None => cwd,
    };
    ctx.calcifer().await?.post("/v1/projects", &Open { path, create }, None).await
}
