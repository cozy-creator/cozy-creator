//! `cozy project`: the CLI's side of Calcifer's projects (xs-136, cl-716). Calcifer finds the
//! project, makes its takes and versions and keeps its assets; these verbs ask it and print, and
//! `render` follows each run as `cozy run` does. Ctrl-C stops following, never the runs.
use std::{path::PathBuf, process::ExitCode};

use protocol::{
    calcifer::{On, local_usable},
    error::{Error, Kind},
    hub::At,
    project::{Approve, Approved, Clean, Event, Import, Imported, Open, Opened, Projects, Rename, Renamed, Rendered, Runs, State, Transferred},
};

use crate::{
    Context, output,
    run::{mint_key, settled, watched},
};

/// A push or pull that moved all it could and left out a file.
const PARTIAL: u8 = 3;

const EXAMPLES: &str = "Examples:
  cozy project new garden
  cd garden && cozy project import ./kyogre.png --character   prints the line to paste into project.yaml
  cozy project render all --on kiyui
  cozy project render fight --new -n 3 --on kiyui    three more takes of fight
  cozy project approve fight 2
  cozy project rename theo mara                after renaming it in project.yaml
  cozy project push                            its files to your Tensorhub account
  cozy project pull                            on another computer, beside project.yaml and project.lock

project.yaml's comments and the README explain the rest.";

const RENDER_EXAMPLES: &str = "Examples:
  cozy project render all --on kiyui               everything missing or out of date, then the edit
  cozy project render all --on kiyui,jitae         spread over two machines
  cozy project render fight --on kiyui             fight and what it reads, if missing or out of date
  cozy project render fight --new --on kiyui       a new take of fight, whatever stands
  cozy project render fight --new -n 3 --on kiyui  three new takes of fight
  cozy project render fight --new --approve --on kiyui
  cozy project render all --new --on kiyui         every node again
  cozy project render fight --in 15 --on kiyui     a new take: the take in use up to about 15 s, the rest new
  cozy project render fight --take 2 --in 15 --on kiyui   the same, keeping take 2
  cozy project render fight --take 2 --on kiyui    a new take: take 2, continued from its end

A node reads as what `approve` or --approve chose, else its newest. A file in its image: or video: pins it: nothing is made.

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
    #[command(alias = "list", after_help = "Example:\n  cozy project ls")]
    Ls,
    Import(ImportFile),
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
    /// Send every file project.lock names that your Tensorhub account lacks, private to it.
    #[command(after_help = "Example:\n  cozy project push   then commit project.yaml and project.lock; assets/ stays out of git\n\nA file over the 5 GiB the Hub holds is skipped with a warning, everything else goes, and it exits 3.\nCtrl-C stops following, never the transfer.")]
    Push,
    /// Fetch every file project.lock names that assets/ lacks, as real files.
    #[command(after_help = "Example:\n  git clone … && cd garden && cozy project pull\n\nA file not at the Hub is skipped with a warning, everything else comes, and it exits 3.\nCtrl-C stops following, never the transfer.")]
    Pull,
}

/// Bring a picture, video or sound into the project, and print the line that names it in project.yaml.
#[derive(clap::Args, Debug)]
#[command(after_help = "Examples:
  cozy project import ./kyogre.png --character
  cozy project import https://example.com/groudon.png --character
  cozy project import ./coast.jpg
  cozy project import ./sword.png --prop

A URL is downloaded here, up to 64 MiB, from a public address.")]
struct ImportFile {
    /// A path, or an http(s) URL.
    source: String,
    /// The picture is a character's: its background is removed, here if this computer runs models, else on
    /// the first machine that renders something reading it.
    #[arg(long, conflicts_with = "prop")]
    character: bool,
    /// The picture is a prop's: its background is removed, as with --character.
    #[arg(long)]
    prop: bool,
}

/// Bring nodes up to date: make only what is missing or out of date, text edits included.
#[derive(clap::Args, Debug)]
#[command(after_help = RENDER_EXAMPLES)]
struct Render {
    /// The nodes (`theo`, `dock.b`, `fight`, `fight.upscale`, …) to bring up to date, with what they read,
    /// or `all`: every node, in order, then the edit.
    #[arg(required = true, value_name = "NAME")]
    names: Vec<String>,
    /// The machines the renders go to: `local`, machine names, or `any`, comma-separated.
    #[arg(long = "on", value_name = "MACHINE", value_delimiter = ',', default_value = "local")]
    on: Vec<String>,
    /// How many takes or versions of each node it renders. A current node stays as it is without --new.
    #[arg(short = 'n', value_name = "N", default_value_t = 1)]
    n: u32,
    /// Render what is current too; an approved version stays chosen over what it makes.
    #[arg(long)]
    new: bool,
    /// Approve the new take or version when it lands (only with -n 1).
    #[arg(long)]
    approve: bool,
    /// With `all`: no upscales; cut only the native preview (`preview.mp4`).
    #[arg(long)]
    preview: bool,
    /// One shot: a new take keeping this take's start, not the one in use. Without --in, it goes on from this take's end.
    #[arg(long, value_name = "N")]
    take: Option<u32>,
    /// One shot: a new take keeping that take up to about here, and rendering what follows with the shot's current text.
    #[arg(long = "in", value_name = "SECONDS", allow_negative_numbers = true, value_parser = seconds)]
    in_s: Option<f64>,
}

/// A time in seconds, any finite number: Calcifer says what it comes to.
fn seconds(text: &str) -> Result<f64, String> {
    text.parse().ok().filter(|at: &f64| at.is_finite()).ok_or_else(|| format!("{text} is not a time in seconds"))
}

pub async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.verb {
        Verb::New { path } => {
            let opened = open(ctx, Some(&path), true).await?;
            let folder = output::link(ctx, &opened.path);
            output::say(ctx, &format!("made {folder}: a short film, ready to render or change in its project.yaml"));
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
        Verb::Import(import) => return self::import(ctx, args.path.as_ref(), import).await,
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
                let approved = match n.approved {
                    _ if n.pinned => "pinned".into(),
                    Some(number) => number.to_string(),
                    None => "-".into(),
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
        Verb::Push => {
            let done = transfer(ctx, args.path.as_ref(), "push", "sent").await?;
            let at = done.kept_in.as_deref().unwrap_or("the Hub");
            let (moved, bytes, rest) = (done.moved, output::bytes(done.bytes), rest(&done));
            output::say(ctx, &format!("{} files at {at}: {moved} sent ({bytes}), {rest} there already", done.files));
            return Ok(skipped(&done));
        }
        Verb::Pull => {
            let done = transfer(ctx, args.path.as_ref(), "pull", "fetched").await?;
            let (moved, bytes, rest) = (done.moved, output::bytes(done.bytes), rest(&done));
            output::say(ctx, &format!("{} files: {moved} fetched ({bytes}), {rest} here already", done.files));
            return Ok(skipped(&done));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The files a transfer had no need to move.
fn rest(done: &Transferred) -> u64 {
    done.files - done.moved - done.skipped.len() as u64
}

/// A warning for each file a transfer left out, and its exit: `PARTIAL` when it left out any.
fn skipped(done: &Transferred) -> ExitCode {
    for file in &done.skipped {
        output::warn(&format!("skipped {}: {}", file.path, file.why));
    }
    if done.skipped.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(PARTIAL) }
}

/// `POST /v1/projects/{id}/<verb>`, followed on the project's events to its end, which `--json` prints.
async fn transfer(ctx: &Context, at: Option<&PathBuf>, verb: &str, moved: &str) -> Result<Transferred, Error> {
    let opened = open(ctx, at, false).await?;
    let calcifer = ctx.calcifer().await?;
    let mut events = calcifer.watch::<Event>(&format!("/v1/projects/{}/events", opened.id)).await?;
    let ask = At { hub: Some(ctx.hub().to_string()) };
    let _: Opened = calcifer.post(&format!("/v1/projects/{}/{verb}", opened.id), &ask, None).await?;
    while let Some(event) = events.next().await {
        match event {
            Event::Moved { file: Some(file), bytes } => output::say(ctx, &format!("{moved} {file} ({} so far)", output::bytes(bytes))),
            Event::Moved { file: None, bytes } => output::say(ctx, &format!("{} so far", output::bytes(bytes))),
            Event::Transferred { error: Some(error), .. } => return Err(error),
            Event::Transferred { done: Some(done), .. } => {
                if ctx.json {
                    output::json(&done);
                }
                return Ok(done);
            }
            _ => {}
        }
    }
    let said = format!("Calcifer stopped before the {verb} ended; run it again and it goes on from what landed");
    Err(Error::new(Kind::Unavailable, "project_events_ended", said).next(format!("cozy project -p {} {verb}", opened.path.display())))
}

/// `POST /v1/projects/{id}/imports`, a clean sent with it followed to its end, and the line to paste.
async fn import(ctx: &Context, at: Option<&PathBuf>, args: ImportFile) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let calcifer = ctx.calcifer().await?;
    let mut events = calcifer.watch::<Event>(&format!("/v1/projects/{}/events", opened.id)).await?;
    let cwd = std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let source = match args.source.contains("://") {
        true => args.source,
        false => protocol::target::spelled(&args.source, &cwd, &ctx.user_home).to_string_lossy().into_owned(),
    };
    let ask = Import { source, clean: args.character || args.prop, hub: ctx.hub().to_string() };
    let mut imported: Imported = calcifer.post(&format!("/v1/projects/{}/imports", opened.id), &ask, None).await?;
    imported.warnings.iter().for_each(|warning| output::warn(warning));
    // `removed` from Calcifer is an earlier clean; one that lands here was made now.
    let mut cleaned_now = false;
    if let Clean::Removing { run } = &imported.clean {
        let followed = watched(ctx, calcifer.run(run).await?).await?;
        imported.clean = match settled(&followed) {
            false => imported.clean,
            true => {
                let mut landed = Clean::Later;
                while let Some(event) = events.next().await {
                    match event {
                        Event::Cleaned { sha256, error } if sha256 == imported.sha256 => {
                            match error {
                                Some(error) => output::warn(&error),
                                None => (landed, cleaned_now) = (Clean::Removed, true),
                            }
                            break;
                        }
                        Event::Ended { .. } => break,
                        _ => {}
                    }
                }
                landed
            }
        };
    }
    if ctx.json {
        output::json(&imported);
        return Ok(ExitCode::SUCCESS);
    }
    let said = match imported.clean {
        Clean::Untouched => "",
        Clean::Removed if cleaned_now => " (background removed)",
        Clean::Removed => " (background already removed)",
        Clean::Removing { .. } => " (its background is being removed)",
        Clean::Later => " (its background is removed when a machine first renders something that reads it)",
    };
    let verb = if imported.already { "already imported" } else { "imported" };
    anstream::println!("{verb} {}{said}", imported.prefix);
    anstream::println!("{}", imported.line);
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
        new: args.new,
        take: args.take,
        in_s: args.in_s,
    };
    let path = format!("/v1/projects/{}/runs", opened.id);
    let _: Opened = calcifer.post(&path, &runs, Some(&mint_key())).await?;
    let status = format!("cozy project -p {} status", opened.path.display());
    let mut failed = std::collections::HashSet::new();
    // Each cut as last said: a render ends on what it holds, not on what it held as a take landed.
    let mut cuts: Vec<(String, String)> = vec![];
    while let Some(event) = events.next().await {
        match event {
            Event::Started { run, .. } => {
                if !settled(&watched(ctx, calcifer.run(&run).await?).await?) {
                    output::say(ctx, "the project's runs go on in Calcifer");
                    output::next(ctx, &[&status]);
                    return Ok(ExitCode::SUCCESS);
                }
            }
            Event::Landed { name, noun, number, path, execution_ms, .. } => {
                let path = output::link(ctx, &opened.path.join(path));
                let took = execution_ms.map(|ms| format!(" in {}", clock(ms))).unwrap_or_default();
                output::say(ctx, &format!("{name}: {noun} {number}{took}  {path}"));
            }
            Event::Said { message } => output::say(ctx, &message),
            Event::Warning { message } => output::warn(&message),
            Event::Failed { error } => {
                output::failure(ctx, &error.message, &error.next);
                failed.insert(error.message);
            }
            Event::Moved { .. } | Event::Transferred { .. } | Event::Cleaned { .. } => {}
            Event::Cut { cut, frames, fps, retimed, unchanged, .. } => {
                let (path, seconds) = (output::link(ctx, &opened.path.join(&cut)), frames as f64 / fps);
                let (name, left) = (cut.trim_end_matches(".mp4"), if unchanged { " (unchanged)" } else { "" });
                let line = format!("{name}: {path}, {seconds:.1} s{}{left}", retimes(&retimed, fps));
                cuts.retain(|(named, _)| *named != cut);
                cuts.push((cut, line));
            }
            Event::Ended { error, rendered } => {
                cuts.iter().for_each(|(_, line)| output::say(ctx, line));
                rendered.iter().for_each(|rendered| output::say(ctx, &finished(rendered)));
                return match error {
                    Some(error) if !ctx.json && failed.contains(&error.message) => Ok(ExitCode::from(error.exit_code() as u8)),
                    Some(error) => Err(error),
                    None => Ok(ExitCode::SUCCESS),
                };
            }
        }
    }
    Err(Error::new(Kind::Unavailable, "project_events_ended", "Calcifer stopped telling how the runs go").next(status))
}

/// `3m52s`, or under a minute `38s`.
fn clock(ms: u64) -> String {
    output::span(((ms + 500) / 1000) as i64)
}

/// A render's last line: what it made and how long it took, with the generation time summed when renders ran at once.
fn finished(rendered: &Rendered) -> String {
    let count = |n: u32, noun: &str| (n > 0).then(|| format!("{n} {noun}{}", if n == 1 { "" } else { "s" }));
    let made: Vec<String> = [count(rendered.takes, "take"), count(rendered.versions, "version")].into_iter().flatten().collect();
    let generation = (rendered.execution_ms > rendered.elapsed_ms).then(|| format!(" ({} of generation)", clock(rendered.execution_ms)));
    format!("rendered {} in {}{}", made.join(" and "), clock(rendered.elapsed_ms), generation.unwrap_or_default())
}

/// Under the table: what is missing, what is out of date and why, and where the cuts are.
fn summary(ctx: &Context, state: &State) {
    for node in state.nodes.iter().filter(|node| !node.prefixes.is_empty()) {
        let made: Vec<String> = node.prefixes.iter().map(|made| format!("{} {} {}", node.noun, made.number, made.prefix)).collect();
        output::say(ctx, &format!("{}: {}", node.name, made.join(", ")));
        for made in &node.prefixes {
            if let Some(kept) = made.kept {
                output::say(ctx, &kept.said(&node.name, made.number));
            }
        }
    }
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
    if state.absent > 0 {
        output::say(ctx, &format!("{} files project.lock names are not on this computer", state.absent));
        output::next(ctx, &["cozy project pull"]);
    } else if !state.missing.is_empty() || !state.stale.is_empty() || uncut {
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
