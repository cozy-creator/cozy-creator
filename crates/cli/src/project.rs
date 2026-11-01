//! `cozy project`: the CLI's side of Calcifer's projects (xs-136, cl-716). Calcifer finds the
//! project, makes its takes and versions and keeps its assets; these verbs ask it and print, and
//! `render` follows each run as `cozy run` does. Ctrl-C stops following, never the runs.
use std::{path::PathBuf, process::ExitCode};

use protocol::{
    calcifer::{On, local_usable},
    error::{Error, Kind},
    project::{Approve, Approved, Event, Open, Opened, Projects, Runs, State},
};

use crate::{
    Context, output,
    run::{follow, mint_key, settled},
};

const EXAMPLES: &str = "Examples:
  cozy project new garden                    a folder with project.yaml, references/ and plates/
  cd garden && cozy project render all --on kiyui
                                             everything missing or out of date, then the edit
  cozy project status                        what is missing, out of date and why, and the cuts
  cozy project render fight -n 3 --on kiyui  three takes of the shot fight
  cozy project approve fight 2               its take 2 approved: what is made from it goes out of date
  cozy project -p ~/films/garden status      a project elsewhere, as git -C
  cozy project ls                            the projects this computer knows

The project is the folder holding project.yaml, found from the folder you are in or above it.
project.yaml lists entities (characters, props, environments with camera setups), world states
and shots; people write it and cozy never does. A shot's alternatives, and its upscale's, are
takes; every other node's are versions. Each is a set of assets under assets/, and project.lock
records them, what made each and which is approved. Keep your own files in references/ (what
guides) and plates/ (footage to edit): project.yaml's paths are relative to the folder. A file
you attach (image:, or a shot's video:) is used until you approve one cozy made. A node's
render: {with, models, lora, options} (or defaults: per kind) says how it is made. Names match
in any case. The edit is edit.mp4, cut on this computer.";

const RENDER_EXAMPLES: &str = "Examples:
  cozy project render all --on kiyui          everything missing or out of date, then the edit
  cozy project render all --on kiyui,jitae    the same, spread over two machines
  cozy project render theo dock.b --on kiyui  a new version of each (a first one is approved)
  cozy project render fight -n 3 --on kiyui   three takes of fight
  cozy project render fight --approve --on kiyui
                                              a new take of fight, approved when it lands
  cozy project render all --preview --on kiyui
                                              everything but upscales, and the native preview.mp4

Each is sent as soon as what it reads is made, so independent ones run at once; with several
machines each goes to the least busy, one that holds its models first. A render asked while
others go joins them. Character and prop pictures are cleaned on this computer, and the edit is
cut here. Ctrl-C stops following, never the runs: `cozy project status` shows them.";

/// A video project: a folder of nodes (characters, props, environments and their camera setups,
/// world states, shots) whose takes and versions `cozy project render` makes.
#[derive(clap::Args, Debug)]
#[command(after_help = EXAMPLES)]
pub struct Args {
    /// The project's folder, or a folder inside it, instead of the one you are in (as git -C).
    #[arg(short = 'p', value_name = "PATH", global = true)]
    path: Option<PathBuf>,
    #[command(subcommand)]
    verb: Verb,
}

#[derive(clap::Subcommand, Debug)]
enum Verb {
    /// Make a project: a folder with a project.yaml to fill in (it shows each kind of entity),
    /// references/ and plates/ for your own files, and assets/ for what cozy makes.
    #[command(after_help = "Example:\n  cozy project new garden && $EDITOR garden/project.yaml")]
    New {
        /// The folder to make it in.
        path: PathBuf,
    },
    /// The projects this computer knows, last used first; a folder that moved shows as missing.
    #[command(after_help = "Example:\n  cozy project ls")]
    Ls,
    Render(Render),
    /// Approve a node's take (a shot's) or version, numbered from 1 as `status` counts them: what is
    /// made from it goes out of date.
    #[command(after_help = "Example:\n  cozy project approve fight 2")]
    Approve {
        /// The node: `theo`, `dock.b` (a setup), `fight`, `fight.upscale`, …
        name: String,
        /// Its take or version, 1 first.
        version: u32,
    },
    /// What is missing, what is out of date and why, and where the cuts are.
    #[command(after_help = "Example:\n  cozy project status")]
    Status,
}

/// New takes and versions of a project's nodes, made on a machine; `all` brings everything up to
/// date.
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
                let assets: Vec<&str> = approved.assets.values().map(String::as_str).collect();
                let (name, noun, number) = (&approved.name, &approved.noun, approved.number);
                output::say(ctx, &format!("{name}: {noun} {number} approved ({})", assets.join(", ")));
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
            let table = output::Table::new(ctx, ["NAME", "KIND", "MADE", "APPROVED", "MADE BY", "STATUS"]);
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
                [n.name.clone(), n.kind.clone(), made, approved, made_by, n.status.clone()]
            });
            rows.fold(table, output::Table::row).print();
            summary(ctx, &state);
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
            Event::Landed { name, noun, number, assets } => {
                let assets: Vec<&str> = assets.values().map(String::as_str).collect();
                output::say(ctx, &format!("{name}: {noun} {number} ({})", assets.join(", ")));
            }
            Event::Said { message } => output::say(ctx, &message),
            Event::Warning { message } => output::warn(&message),
            Event::Cut { cut, seconds } => {
                let path = output::link(ctx, &opened.path.join(&cut));
                output::say(ctx, &format!("cut {path} in {seconds:.1} s"));
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
    for cut in &state.cuts {
        let skips = match cut.skips.as_slice() {
            [] => String::new(),
            skips => format!("; it skips {} (nothing made yet)", skips.join(", ")),
        };
        let name = cut.cut.trim_end_matches(".mp4");
        match cut.cut_at_ms {
            Some(at_ms) => output::say(
                ctx,
                &format!("{name}: {}, cut {} ago{skips}", output::link(ctx, &cut.path), output::age(at_ms)),
            ),
            None => output::say(ctx, &format!("{name}: not cut yet ({})", cut.path.display())),
        }
    }
    let uncut = state.cuts.iter().any(|cut| cut.cut_at_ms.is_none());
    if !state.missing.is_empty() || !state.stale.is_empty() || uncut {
        output::next(ctx, &["cozy project render all --on <machine>"]);
    }
}

/// The project holding `at` (else the folder you are in) as Calcifer opens it, made there first
/// when `create`.
async fn open(ctx: &Context, at: Option<&PathBuf>, create: bool) -> Result<Opened, Error> {
    let cwd = std::env::current_dir()
        .map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let path = match at {
        Some(at) => crate::run::target::spelled(&at.to_string_lossy(), &cwd, &ctx.user_home),
        None => cwd,
    };
    ctx.calcifer().await?.post("/v1/projects", &Open { path, create }, None).await
}
