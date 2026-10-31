//! `cozy project` and `cozy render`: the CLI's side of Calcifer's projects (xs-136). Calcifer makes
//! a project's versions and keeps its files; these ask it and follow each run as `cozy run` does.
//! Ctrl-C stops following, never the runs.
use std::{path::PathBuf, process::ExitCode};

use protocol::{
    calcifer::{On, local_usable},
    error::{Error, Kind},
    project::{Approve, Approved, Event, Open, Opened, Runs, State},
};

use crate::{
    Context, output,
    run::{follow, settled},
};

const PROJECT_EXAMPLES: &str = "Examples:
  cozy project new --project garden          a folder with a project.yaml to fill in
  cozy render all --project garden --on kiyui
                                             everything missing or out of date, then the edit
  cozy project status --project garden       what is missing, out of date and why, and the edit
  cozy render theo --project garden --on kiyui && cozy project approve theo 2 --project garden
                                             a second take of theo, then approve it

project.yaml lists entities (characters, props, environments with camera setups), world states
and shots, and people write it; cozy never does. Each version is a file under files/, and
project.lock records each node's versions, what made each, and which is approved. A file you
attach (image:, or a shot's video:) is used until you approve a version cozy made. A node's
render: {with, models, lora, options} (or defaults: per kind) says how it is made. Names match
in any case. The edit is <project>/edit.mp4, cut on this computer.";

const RENDER_EXAMPLES: &str = "Examples:
  cozy render all --project garden --on kiyui   everything missing or out of date, then the edit
  cozy render all --project garden --on kiyui,jitae
                                                the same, spread over two machines
  cozy render theo dock.b --project garden --on kiyui
                                                a new version of each (a first one is approved)
  cozy render fight -n 3 --on kiyui             three takes of fight, from inside the project's folder
  cozy render all --preview --on kiyui          everything but upscales, and the native preview.mp4

Each version is sent as soon as what it reads is made, so independent ones run at once; with
several machines each goes to the least busy, one that holds its models first. A render asked
while others go joins them. Character and prop pictures are cleaned on this computer, and the
edit is cut here. Ctrl-C stops following, never the runs: `cozy project status` shows them.";

/// A video project: a folder of nodes (characters, props, environments and their camera setups,
/// world states, shots) whose versions `cozy render` makes.
#[derive(clap::Args, Debug)]
#[command(after_help = PROJECT_EXAMPLES)]
pub struct Args {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(clap::Subcommand, Debug)]
enum Verb {
    /// Make a video project: a folder with a project.yaml to fill in (it shows each kind).
    #[command(after_help = "Example:\n  cozy project new --project garden && $EDITOR garden/project.yaml")]
    New {
        #[command(flatten)]
        at: At,
    },
    /// Approve version N of a node: what is made from it goes out of date.
    #[command(
        after_help = "Example:\n  cozy project approve theo 2 --project garden   (versions are numbered as `cozy project status` lists them)"
    )]
    Approve {
        /// The node: `theo`, `dock.b` (a setup), `fight.upscale`, …
        name: String,
        /// Its version, 1 first.
        version: u32,
        #[command(flatten)]
        at: At,
    },
    /// What is missing, what is out of date and why, and where the edit is.
    #[command(after_help = "Example:\n  cozy project status --project garden")]
    Status {
        #[command(flatten)]
        at: At,
    },
}

#[derive(clap::Args, Debug)]
struct At {
    /// The project's folder.
    #[arg(long, value_name = "PATH", default_value = ".")]
    project: PathBuf,
}

/// New versions of a project's nodes, made on a machine; `all` brings everything up to date.
#[derive(clap::Args, Debug)]
#[command(after_help = RENDER_EXAMPLES)]
pub struct Render {
    /// The nodes (`theo`, `dock.b`, `fight`, `fight.upscale`, …) to make new versions of, or
    /// `all`: everything missing or out of date, in order, each new version approved, then the edit.
    #[arg(required = true, value_name = "NAME")]
    names: Vec<String>,
    #[command(flatten)]
    at: At,
    /// The machines the renders go to: `local`, machine names, or `any`, comma-separated.
    #[arg(long = "on", value_name = "MACHINE", value_delimiter = ',', default_value = "local")]
    on: Vec<String>,
    /// New versions of each.
    #[arg(short = 'n', value_name = "N", default_value_t = 1)]
    n: u32,
    /// With `all`: no upscales; cut only the native preview (`preview.mp4`).
    #[arg(long)]
    preview: bool,
}

pub async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.verb {
        Verb::New { at } => {
            let opened = open(ctx, &at, true).await?;
            output::say(
                ctx,
                &format!("made {}: add assets, world states and shots to its project.yaml", opened.path.display()),
            );
            let render = format!("cozy render all --project {} --on <machine>", opened.path.display());
            output::next(ctx, &[&render]);
        }
        Verb::Approve { name, version, at } => {
            let opened = open(ctx, &at, false).await?;
            let path = format!("/v1/projects/{}/approve", opened.id);
            let approved: Approved =
                ctx.calcifer().await?.post(&path, &Approve { name: name.clone(), version }, None).await?;
            approved.warnings.iter().for_each(|warning| output::warn(warning));
            if ctx.json {
                output::json(&approved);
            } else {
                output::say(ctx, &format!("{name}: version {version} approved ({})", approved.file));
                if !approved.stale.is_empty() {
                    output::say(ctx, &format!("out of date now: {}", approved.stale.join(", ")));
                }
            }
        }
        Verb::Status { at } => {
            let opened = open(ctx, &at, false).await?;
            let state: State = ctx.calcifer().await?.get(&format!("/v1/projects/{}", opened.id)).await?;
            state.warnings.iter().for_each(|warning| output::warn(warning));
            if ctx.json {
                output::json(&state);
                return Ok(ExitCode::SUCCESS);
            }
            let chosen = |n: &protocol::project::Node| match (n.chosen, &n.file) {
                (Some(v), _) => v.to_string(),
                (None, Some(_)) => "attached".into(),
                (None, None) => "-".into(),
            };
            let table = output::Table::new(ctx, ["NAME", "KIND", "VERSIONS", "APPROVED", "MADE BY", "STATUS"]);
            let rows = state.nodes.iter().map(|n| {
                let made_by = n.made_by.clone().unwrap_or_else(|| "-".into());
                [n.name.clone(), n.kind.clone(), n.versions.to_string(), chosen(n), made_by, n.status.clone()]
            });
            rows.fold(table, output::Table::row).print();
            summary(ctx, &state);
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub async fn render(ctx: &Context, args: Render) -> Result<ExitCode, Error> {
    let opened = open(ctx, &args.at, false).await?;
    if let [only] = args.names.as_slice()
        && only.eq_ignore_ascii_case("all")
    {
        // Calcifer cuts the edit with the ffmpeg cozy keeps; fetched now if this computer has none.
        crate::tools::ffmpeg(ctx).await?;
    }
    let calcifer = ctx.calcifer().await?;
    let mut events = calcifer.watch::<Event>(&format!("/v1/projects/{}/events", opened.id)).await?;
    let on = args.on.into_iter().map(|on| local_usable(&on).map(|()| On::from(on))).collect::<Result<_, _>>()?;
    let runs = Runs { names: args.names, n: args.n, on, hub: ctx.hub().to_string(), preview: args.preview };
    let _: Opened = calcifer.post(&format!("/v1/projects/{}/runs", opened.id), &runs, None).await?;
    while let Some(event) = events.next().await {
        match event {
            Event::Started { run, .. } => {
                if !settled(&follow(ctx, calcifer.run(&run).await?).await?) {
                    let status = format!("cozy project status --project {}", opened.path.display());
                    output::say(ctx, "the project's runs go on in Calcifer");
                    output::next(ctx, &[&status]);
                    return Ok(ExitCode::SUCCESS);
                }
            }
            Event::Landed { name, file } => output::say(ctx, &format!("{name}: {file}")),
            Event::Said { message } => output::say(ctx, &message),
            Event::Warning { message } => output::warn(&message),
            Event::Cut { path, seconds } => {
                output::say(ctx, &format!("cut {} in {seconds:.1} s", output::link(ctx, &path)));
            }
            Event::Ended { error: Some(error) } => return Err(error),
            Event::Ended { error: None } => return Ok(ExitCode::SUCCESS),
        }
    }
    Err(Error::new(Kind::Unavailable, "project_events_ended", "Calcifer stopped telling how the runs go")
        .next(format!("cozy project status --project {}", opened.path.display())))
}

/// Under the table: what is missing, what is out of date and why, and where the edit is.
fn summary(ctx: &Context, state: &State) {
    let at = state.path.display();
    let named = |test: &dyn Fn(&str) -> bool| -> Vec<String> {
        state.nodes.iter().filter(|n| test(&n.status)).map(|n| n.name.clone()).collect()
    };
    let missing = named(&|s| s == "no version yet" || s == "picture not cleaned yet");
    let stale: Vec<String> = (state.nodes.iter())
        .filter_map(|e| e.status.strip_prefix("out of date: ").map(|why| format!("{} ({why})", e.name)))
        .collect();
    let render = format!("cozy render all --project {at} --on <machine>");
    if !missing.is_empty() {
        output::say(ctx, &format!("missing: {}", missing.join(", ")));
    }
    if !stale.is_empty() {
        output::say(ctx, &format!("out of date: {}", stale.join(", ")));
    }
    let edit = &state.edit;
    let skips = match edit.skips.as_slice() {
        [] => String::new(),
        skips => format!("; it skips {} (no version yet)", skips.join(", ")),
    };
    match edit.cut_at_ms {
        Some(at_ms) => {
            output::say(ctx, &format!("edit: {}, cut {} ago{skips}", output::link(ctx, &edit.path), output::age(at_ms)))
        }
        None => output::say(ctx, &format!("edit: not cut yet ({})", edit.path.display())),
    }
    if !missing.is_empty() || !stale.is_empty() || edit.cut_at_ms.is_none() {
        output::next(ctx, &[&render]);
    }
}

/// The project at `at` as Calcifer opens it (made first when `create`).
async fn open(ctx: &Context, at: &At, create: bool) -> Result<Opened, Error> {
    let cwd = std::env::current_dir()
        .map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let path = crate::run::target::spelled(&at.project.to_string_lossy(), &cwd, &ctx.user_home);
    ctx.calcifer().await?.post("/v1/projects", &Open { path, create }, None).await
}
