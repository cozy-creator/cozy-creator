//! `cozy project` and `cozy render`: the CLI's side of Calcifer's projects (xs-136). Calcifer makes
//! a project's versions and keeps its files; these ask it and follow each run as `cozy run` does.
//! Ctrl-C stops following, never the runs.
use std::{path::PathBuf, process::ExitCode};

use protocol::{
    error::{Error, Kind},
    project::{Approve, Approved, Event, Open, Opened, Runs, State},
};

use crate::{
    Context, output,
    run::{Placement, follow, settled},
};

const PROJECT_EXAMPLES: &str = "Examples:
  cozy project new --project garden          a folder with a project.yaml to fill in
  cozy render all --project garden --on kiyui
                                             everything missing or out of date, then the edit
  cozy project status --project garden       what is missing, out of date and why, and the edit
  cozy render theo --project garden --on kiyui && cozy project approve theo 2 --project garden
                                             a second take of theo, then choose it

project.yaml lists assets (characters, props, environments with camera setups), world states
and shots; each version is a file under files/, and the chosen one is the entity's image: or
video:. Names match in any case. The edit is <project>/edit.mp4, cut on this computer.";

const RENDER_EXAMPLES: &str = "Examples:
  cozy render all --project garden --on kiyui   everything missing or out of date, in order, then the edit
  cozy render theo dock.b --project garden --on kiyui
                                                a new version of each (a first one is chosen)
  cozy render fight -n 3 --on kiyui             three takes of fight, from inside the project's folder

Renders run on --on's machine; character and prop pictures are cleaned on this computer, and the
edit is cut here. Ctrl-C stops following, never the runs: `cozy project status` shows them.";

/// A video project: a folder of entities (characters, props, environments and their camera setups,
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
    /// Choose version N of an entity: what is made from it goes out of date.
    #[command(
        after_help = "Example:\n  cozy project approve theo 2 --project garden   (versions are numbered as `cozy project status` lists them)"
    )]
    Approve {
        /// The entity: `theo`, `dock.b` (a setup), `fight.upscale`, …
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

/// New versions of a project's entities, made on a machine; `all` brings everything up to date.
#[derive(clap::Args, Debug)]
#[command(after_help = RENDER_EXAMPLES)]
pub struct Render {
    /// The entities (`theo`, `dock.b`, `fight`, `fight.upscale`, …) to make new versions of, or
    /// `all`: everything missing or out of date, in order, each new version chosen, then the edit.
    #[arg(required = true, value_name = "NAME")]
    names: Vec<String>,
    #[command(flatten)]
    at: At,
    #[command(flatten)]
    placement: Placement,
    /// New versions of each.
    #[arg(short = 'n', value_name = "N", default_value_t = 1)]
    n: u32,
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
                output::say(ctx, &format!("{name}: version {version} chosen ({})", approved.file));
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
            let chosen = |e: &protocol::project::Entity| match (e.chosen, &e.file) {
                (Some(n), _) => n.to_string(),
                (None, Some(_)) => "attached".into(),
                (None, None) => "-".into(),
            };
            let table = output::Table::new(ctx, ["NAME", "KIND", "VERSIONS", "CHOSEN", "STATUS"]);
            let rows = state
                .entities
                .iter()
                .map(|e| [e.name.clone(), e.kind.clone(), e.versions.to_string(), chosen(e), e.status.clone()]);
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
    let runs = Runs { names: args.names, n: args.n, on: args.placement.on()?, hub: ctx.hub().to_string() };
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
        state.entities.iter().filter(|e| test(&e.status)).map(|e| e.name.clone()).collect()
    };
    let missing = named(&|s| s == "no version yet" || s == "picture not cleaned yet");
    let stale: Vec<String> = (state.entities.iter())
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
