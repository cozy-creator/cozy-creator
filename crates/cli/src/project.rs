//! `cozy project`: the CLI's side of Calcifer's projects (xs-151, cl-758). Calcifer finds the
//! project, makes its assets' takes and versions and keeps their files; these verbs ask it and print,
//! and `render` draws what Calcifer says of it (`render.rs`). Ctrl-C stops following, never the runs.
mod assets;
pub(crate) mod render;

use std::{path::PathBuf, process::ExitCode};

use protocol::{
    calcifer::{On, local_usable},
    error::{Error, Kind},
    hub::At,
    project::{
        Approve, Approved, Asked, By, Cancel, Cancelled, Event, History, HistoryStep, HistoryStepped, Import, Imported, NewVersion, Open, Opened, Projects, Reclaim, Reclaimed, Rename, Renamed, Runs,
        Standing, State, Transferred, Unpin, Unpinned, VersionMoved,
    },
};

use crate::{
    Context, output,
    run::{exit_code, mint_key, settled, watched},
};

/// A push or pull that moved all it could and left out a file.
const PARTIAL: u8 = 3;

const EXAMPLES: &str = "Examples:
  cozy project new garden
  cozy open garden                             in your browser
  cd garden && cozy project import ./kyogre.png   adds it to project.yaml as kyogre, which <kyogre> reads in any text
  cozy project add qwen-image-2/generate_turbo whale instructions='A whale.'   as dropping that tool on the canvas
  cozy project set whale megapixels=1 at=400,120   as its box's controls; --input takes a file of them
  cozy project delete whale                    prints the command that puts it back
  cozy project render all --on kiyui
  cozy project render fight -n 3 --on kiyui    three more takes of fight
  cozy project approve fight 2                 fight holds that file, as its `approved` in project.yaml
  cozy project unapprove fight                  it follows its inputs again
  cozy project rename theo mara                in project.yaml, its <mentions> and its versions
  cozy project history                         each edit, by the page, the CLI or a hand; `undo` and `redo` take one back or do it again
  cozy project reclaim --yes                   deletes what project.yaml no longer names
  cozy project push                            its files to your Tensorhub account
  cozy project pull                            on another computer, beside project.yaml and project.lock

project.yaml's comments and the README explain the rest.";

const RENDER_EXAMPLES: &str = "Examples:
  cozy project render all --on kiyui               a new version of everything not approved, each reading the new ones before it
  cozy project render all --on kiyui,jitae         spread over two machines
  cozy project render all --missing --on kiyui     only what has no state: continues a render that half failed
  cozy project render fight --on kiyui             a new take of fight, and of what it reads only if that has no state
  cozy project render fight -n 3 --on kiyui        three new takes of fight; approved, what it approved stays
  cozy project render fight --approve --on kiyui   a new take, approved as it lands
  cozy project render all --outdated --on kiyui    and of each approved asset whose inputs changed, as its turn comes: the new one takes its place
  cozy project render fight --in 15 --on kiyui     a new take: the take in use up to about 15 s, the rest new
  cozy project render fight --take 2 --in 15 --on kiyui   the same, keeping take 2
  cozy project render fight --take 2 --on kiyui    a new take: take 2, continued from its end

An asset's state is the hash in its image:, video: or audio: (`approve` writes that line, `unapprove` removes it), else its newest
version with these inputs, else none. Reverting a text returns the old output with no run. Every render makes new versions, except of what is approved and not named.

Ctrl-C stops following, never the runs.";

/// A project: a folder of assets, each a stable name holding a set of files, whose takes and versions
/// `cozy project render` makes.
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
    Add(assets::Add),
    Set(assets::Set),
    /// Delete assets, as the X on a box: their work stops, what they made stays until `reclaim`, and it prints what puts each back.
    /// With `--version`, discards takes of one asset instead.
    #[command(after_help = "Examples:\n  cozy project delete whale\n  cozy project restore whale eyJ0ZXh0Ijoi…   the line it printed\n  cozy project delete whale --version 2 --version 3")]
    Delete {
        /// The assets.
        #[arg(required = true, value_name = "NAME")]
        names: Vec<String>,
        /// Discard this take or version: it's off the asset, its number is never used again, and its files stay until `reclaim`. Repeatable.
        #[arg(long = "version", value_name = "N")]
        versions: Vec<u32>,
    },
    /// Put back a deleted asset as it was, where it was, with what it made, or with `--version` a take it discarded.
    #[command(after_help = "Examples:\n  cozy project restore whale eyJ0ZXh0Ijoi…   the line `delete` printed\n  cozy project restore whale --version 2")]
    Restore {
        /// The asset.
        name: String,
        /// What `delete` printed for it.
        #[arg(value_name = "KEPT", required_unless_present = "versions", conflicts_with = "versions")]
        kept: Option<String>,
        /// The take or version to put back. Repeatable.
        #[arg(long = "version", value_name = "N")]
        versions: Vec<u32>,
        /// And its approval, as `delete --version` printed.
        #[arg(long, requires = "versions")]
        approved: bool,
    },
    Render(Render),
    /// Approve a take or version: its hash goes on the asset's line in project.yaml.
    #[command(after_help = "Examples:\n  cozy project approve fight 2\n  cozy project approve kyogre 65b436d4f9a3   any file of the project's, by hash\n\nIt writes `video: <hash>` (or `image:`) into fight in project.yaml, changing nothing else. `cozy project unapprove fight` removes it.")]
    Approve {
        /// The asset: `theo`, `dock_b`, `fight`, `fight_upscale`, …
        name: String,
        /// Its take or version, 1 first, or a hash prefix of any file in the project's `files/`.
        version: String,
    },
    /// Drop assets' queued work and cancel their runs, or `all`.
    #[command(after_help = "Examples:\n  cozy project cancel clash\n  cozy project cancel all")]
    Cancel {
        /// The assets, or `all`.
        #[arg(required = true, value_name = "NAME")]
        names: Vec<String>,
    },
    /// Take back an asset's approval: its line in project.yaml goes, and its state follows its inputs again.
    #[command(after_help = "Example:\n  cozy project unapprove fight")]
    Unapprove {
        /// The asset.
        name: String,
    },
    /// What has no state and why, and each asset's file; with a name, that asset's fields and each take or version.
    #[command(after_help = "Examples:\n  cozy project status\n  cozy project status fight")]
    Status {
        /// One asset.
        name: Option<String>,
    },
    /// Follow the render going to its end, as `render` follows its own: free, it starts nothing. With none going, the project as it stands.
    #[command(after_help = "Example:\n  cozy project render clash --on corteo --queue\n  cozy project wait\n\nExits 0 done, 1 something failed, 4 a run stopped moving: it goes on until you cancel it. Ctrl-C stops waiting, never the render.")]
    Wait,
    /// What was done to the project, newest first: each edit by the page, the CLI or a hand in project.yaml, and whether it's undone.
    #[command(after_help = "Example:\n  cozy project history\n  cozy project undo 42   that entry, if nothing since changed what it did")]
    History {
        /// How many, newest first.
        #[arg(short = 'n', value_name = "N", default_value_t = 20)]
        limit: usize,
    },
    /// Undo the project's newest edit, whoever made it: the page, the CLI, or a hand in project.yaml. Refused if a later edit changed the same thing.
    /// Renders aren't undone, and nothing is rented or ended.
    #[command(after_help = "Examples:\n  cozy project undo\n  cozy project undo 42        that entry of `cozy project history`\n  cozy project undo --to 40   every edit back to 40")]
    Undo {
        /// The entry, from `cozy project history`.
        #[arg(value_name = "ID")]
        entry: Option<u64>,
        /// Every edit back to this one.
        #[arg(long, value_name = "ID", conflicts_with = "entry")]
        to: Option<u64>,
    },
    /// Do again what undo last took back, unless an edit came since; with an ID, that entry.
    #[command(after_help = "Examples:\n  cozy project redo\n  cozy project redo 42\n  cozy project redo --to 45   every edit undone, up to 45")]
    Redo {
        #[arg(value_name = "ID")]
        entry: Option<u64>,
        #[arg(long, value_name = "ID", conflicts_with = "entry")]
        to: Option<u64>,
    },
    /// Keep an asset's versions after renaming it in project.yaml.
    #[command(after_help = "Example:\n  cozy project rename theo mara   (status suggests it after a rename)")]
    Rename {
        /// Its name now, or in project.lock if project.yaml has moved on.
        from: String,
        /// Its new name.
        to: String,
    },
    /// List what project.yaml no longer names, and with --yes delete it: its versions, and any file nothing else names.
    #[command(after_help = "Example:\n  cozy project reclaim\n  cozy project reclaim --yes\n\nNothing is ever deleted without it.")]
    Reclaim {
        /// Delete them.
        #[arg(long)]
        yes: bool,
    },
    /// Send every file project.lock names that your Tensorhub account lacks, private to it.
    #[command(after_help = "Example:\n  cozy project push   then commit project.yaml and project.lock; files/ stays out of git\n\nA file over 5 GiB goes as parts, which project.lock lists. A file on neither this computer nor the Hub is skipped with a warning, everything else goes, and it exits 3.\nCtrl-C stops following, never the transfer.")]
    Push,
    /// Fetch every file project.lock names that files/ lacks, as real files.
    #[command(after_help = "Example:\n  git clone … && cd garden && cozy project pull\n\nA file not at the Hub is skipped with a warning, everything else comes, and it exits 3.\nCtrl-C stops following, never the transfer.")]
    Pull,
    /// Move a project of format 2 to format 3: nothing goes out of date and nothing runs.
    #[command(after_help = "Example:\n  cozy project upgrade\n\nEach asset's state is worked out from the old files, as the cozy that reads format 2 did, and kept.\nThe old files stay beside the new, as project.yaml.v2 and project.lock.v2. Comments stay only there.")]
    Upgrade {
        /// What `cozy project status --json` said, with the cozy that reads format 2.
        #[arg(long, value_name = "FILE")]
        status: Option<PathBuf>,
    },
}

/// Add a picture, video or sound to the project as an element with a name, and print the name it got.
#[derive(clap::Args, Debug)]
#[command(after_help = "Examples:
  cozy project import ./kyogre.png               prints kyogre
  cozy project import ./kyogre.png kyogre_art    prints kyogre_art
  cozy project import https://example.com/groudon.png
  cozy project import 65b436d4f9a3 kyogre --description 'The colossal blue whale.'    a file of the project's, as an asset of its own
  cozy project import ./kyogre2.png --into kyogre    the next version of kyogre: <kyogre.2>

A name something in the project has takes _2, and the same file twice is two elements and one file: <name> reads it in any text.
A URL is downloaded here, up to 64 MiB, from a public address.")]
struct ImportFile {
    /// A path, an http(s) URL, or a file of the project's by the start of its hash, as a take lifted out of what made it.
    source: String,
    /// What to call it. Default: the file's name, lower-cased.
    name: Option<String>,
    /// The words for whatever mentions it.
    #[arg(long)]
    description: Option<String>,
    /// Join this box as its next version, instead of an asset of its own: prints `<box>.<number>`.
    #[arg(long, value_name = "BOX", conflicts_with_all = ["name", "description"])]
    into: Option<String>,
}

/// For each asset named and what it reads, if it has no state, run its operation. Nothing else runs.
#[derive(clap::Args, Debug)]
#[command(after_help = RENDER_EXAMPLES)]
struct Render {
    /// The assets (`theo`, `dock_b`, `fight`, `fight_upscale`, …), with what they read, or `all`: every asset.
    #[arg(required = true, value_name = "NAME")]
    names: Vec<String>,
    /// The machines the renders go to: `local`, machine names, or `any`, comma-separated.
    #[arg(long = "on", value_name = "MACHINE", value_delimiter = ',')]
    on: Vec<String>,
    /// Ask and return at once: what it queued and what's ahead of each is printed, and `status` shows how it goes.
    #[arg(long)]
    queue: bool,
    /// How many new takes or versions of each asset named. `all` makes one of each.
    #[arg(short = 'n', value_name = "N", default_value_t = 1)]
    n: u32,
    /// Only what has no state, rather than a new version.
    #[arg(long)]
    missing: bool,
    /// With all: also a new version of each approved asset out of date when its turn comes. Its approval goes when that lands, and the version it held stays.
    #[arg(long)]
    outdated: bool,
    /// Approve the new take or version when it lands (only with -n 1).
    #[arg(long)]
    approve: bool,
    /// One shot: a new take keeping this take's start, not the one in use. Without --in, it goes on from this take's end.
    #[arg(long, value_name = "N")]
    take: Option<u32>,
    /// One shot: a new take keeping that take up to about here, and rendering what follows with the shot's current text.
    #[arg(long = "in", value_name = "SECONDS", allow_negative_numbers = true, value_parser = seconds)]
    in_s: Option<f64>,
    /// One line per event, each starting with its asset's name, rather than the live view. Off a terminal it always is.
    #[arg(long)]
    plain: bool,
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
            output::nexts(ctx, &opened.next);
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
        Verb::Upgrade { status } => return upgrade(ctx, args.path.as_ref(), status.as_ref()),
        Verb::Add(add) => return assets::add(ctx, args.path.as_ref(), add).await,
        Verb::Set(set) => return assets::set(ctx, args.path.as_ref(), set).await,
        Verb::Delete { names, versions } => return assets::delete(ctx, args.path.as_ref(), names, versions).await,
        Verb::Restore { name, kept, versions, approved } => return assets::restore(ctx, args.path.as_ref(), name, kept, versions, approved).await,
        Verb::Render(render) => return self::render(ctx, args.path.as_ref(), render).await,
        Verb::Approve { name, version } => {
            let opened = open(ctx, args.path.as_ref(), false).await?;
            let path = format!("/v1/projects/{}/approve", opened.id);
            let approved: Approved = ctx.calcifer().await?.post(&path, &Approve { name: name.clone(), version }, None).await?;
            approved.warnings.iter().for_each(|warning| output::warn(warning));
            if ctx.json {
                output::json(&approved);
            } else {
                let (name, noun, number) = (&approved.name, &approved.noun, approved.number);
                let path = output::link(ctx, &opened.path.join(&approved.path));
                match number {
                    0 => output::say(ctx, &format!("{name}: approved in project.yaml ({path})")),
                    number => output::say(ctx, &format!("{name}: {noun} {number} approved in project.yaml ({path})")),
                }
                if !approved.stale.is_empty() {
                    output::say(ctx, &format!("out of date now: {}", approved.stale.join(", ")));
                }
            }
        }
        Verb::Unapprove { name } => {
            let opened = open(ctx, args.path.as_ref(), false).await?;
            let path = format!("/v1/projects/{}/unpin", opened.id);
            let unpinned: Unpinned = ctx.calcifer().await?.post(&path, &Unpin { name }, None).await?;
            if ctx.json {
                output::json(&unpinned);
            } else {
                match unpinned.version {
                    Some(version) => output::say(ctx, &format!("{}: no longer approved at {version}", unpinned.name)),
                    None => output::say(ctx, &format!("{}: no longer approved", unpinned.name)),
                }
                if !unpinned.stale.is_empty() {
                    output::say(ctx, &format!("out of date now: {}", unpinned.stale.join(", ")));
                }
            }
        }
        Verb::Cancel { names } => {
            let opened = open(ctx, args.path.as_ref(), false).await?;
            let path = format!("/v1/projects/{}/cancel", opened.id);
            let cancelled: Cancelled = ctx.calcifer().await?.post(&path, &Cancel { names }, None).await?;
            if ctx.json {
                output::json(&cancelled);
                return Ok(ExitCode::SUCCESS);
            }
            cancelled.dropped.iter().for_each(|name| output::say(ctx, &format!("dropped {name} from the queue")));
            cancelled.canceled.iter().for_each(|run| output::say(ctx, &format!("canceled run {run}")));
            if cancelled.dropped.is_empty() && cancelled.canceled.is_empty() {
                output::say(ctx, "nothing of that was queued or running");
            }
        }
        Verb::Reclaim { yes } => {
            let opened = open(ctx, args.path.as_ref(), false).await?;
            let path = format!("/v1/projects/{}/reclaim", opened.id);
            let reclaimed: Reclaimed = ctx.calcifer().await?.post(&path, &Reclaim { yes }, None).await?;
            if ctx.json {
                output::json(&reclaimed);
                return Ok(ExitCode::SUCCESS);
            }
            for orphan in &reclaimed.orphans {
                output::say(ctx, &format!("{}: {} made, {} only it names, which project.yaml no longer names", orphan.name, orphan.versions, output::bytes(orphan.bytes)));
            }
            let (files, bytes) = (reclaimed.files, output::bytes(reclaimed.bytes));
            match (reclaimed.deleted, files) {
                (true, _) => output::say(ctx, &format!("deleted {files} files, {bytes}")),
                (false, 0) if reclaimed.orphans.is_empty() => output::say(ctx, "nothing to reclaim"),
                (false, _) => output::say(ctx, &format!("{files} files nothing else names, {bytes}")),
            }
            if reclaimed.unrecorded > 0 {
                output::say(ctx, &format!("{} with no record here, which --yes leaves: they may be all that's left of a take", reclaimed.unrecorded));
            }
            if let Some(lost) = &reclaimed.lost {
                output::say(ctx, &format!("{lost}: --yes is refused"));
            }
            output::nexts(ctx, &reclaimed.next);
        }
        Verb::Status { name: Some(name) } => return assets::one(ctx, args.path.as_ref(), name).await,
        Verb::Status { name: None } => return status(ctx, args.path.as_ref()).await,
        Verb::Wait => return wait(ctx, args.path.as_ref()).await,
        Verb::Rename { from, to } => {
            let opened = open(ctx, args.path.as_ref(), false).await?;
            let path = format!("/v1/projects/{}/rename", opened.id);
            let renamed: Renamed = ctx.calcifer().await?.post(&path, &Rename { from, to }, None).await?;
            match ctx.json {
                true => output::json(&renamed),
                false if renamed.versions == 0 => output::say(ctx, &format!("{} is {} now", renamed.from, renamed.to)),
                false => output::say(ctx, &format!("{} is {} now: its {} kept", renamed.from, renamed.to, renamed.versions)),
            }
        }
        Verb::History { limit } => return history(ctx, args.path.as_ref(), limit).await,
        Verb::Undo { entry, to } => return step(ctx, args.path.as_ref(), "undo", HistoryStep { entry, to, edit: None }).await,
        Verb::Redo { entry, to } => return step(ctx, args.path.as_ref(), "redo", HistoryStep { entry, to, edit: None }).await,
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

/// Who made an edit, as the history lists it.
fn by(by: &By) -> String {
    match (by.via.as_str(), &by.client) {
        ("page", _) => "page".to_owned(),
        ("file", _) => "project.yaml".to_owned(),
        (via, Some(client)) => format!("{via} {client}"),
        (via, None) => via.to_owned(),
    }
}

/// `GET /v1/projects/{id}/history`.
async fn history(ctx: &Context, at: Option<&PathBuf>, limit: usize) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let listed: History = ctx.calcifer().await?.get(&format!("/v1/projects/{}/history?limit={limit}", opened.id)).await?;
    if ctx.json {
        output::json(&listed);
        return Ok(ExitCode::SUCCESS);
    }
    if listed.entries.is_empty() {
        output::say(ctx, "no edits yet");
        return Ok(ExitCode::SUCCESS);
    }
    let table = output::Table::new(ctx, ["ID", "WHEN", "BY", "WHAT"]);
    let rows = listed.entries.iter().map(|entry| {
        let what = match &entry.undone {
            Some(undone) => format!("{} (undone {} ago)", entry.words, output::age(undone.at_ms)),
            None => entry.words.clone(),
        };
        [entry.id.to_string(), format!("{} ago", output::age(entry.at_ms)), by(&entry.by), what]
    });
    rows.fold(table, output::Table::row).print();
    let next: Vec<String> = [listed.undo.map(|_| "cozy project undo".to_owned()), listed.redo.map(|_| "cozy project redo".to_owned())].into_iter().flatten().collect();
    output::nexts(ctx, &next);
    Ok(ExitCode::SUCCESS)
}

/// `POST /v1/projects/{id}/history/<verb>`, and what it did.
async fn step(ctx: &Context, at: Option<&PathBuf>, verb: &str, asked: HistoryStep) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let stepped: HistoryStepped = ctx.calcifer().await?.post(&format!("/v1/projects/{}/history/{verb}", opened.id), &asked, None).await?;
    if ctx.json {
        output::json(&stepped);
        return Ok(ExitCode::SUCCESS);
    }
    let done = if verb == "undo" { "undid" } else { "redid" };
    if stepped.already {
        output::say(ctx, &format!("nothing to {verb}: that's {done} already"));
    }
    for entry in &stepped.entries {
        output::say(ctx, &format!("{done} {}: {}", entry.id, entry.words));
    }
    if let Some(stopped) = &stepped.stopped {
        output::warn(&stopped.message);
        output::nexts(ctx, &stopped.next);
        return Ok(ExitCode::FAILURE);
    }
    let next: Vec<String> = match verb {
        "undo" => stepped.entries.last().map(|entry| format!("cozy project redo {}", entry.id)).into_iter().collect(),
        _ => stepped.entries.last().map(|entry| format!("cozy project undo {}", entry.id)).into_iter().collect(),
    };
    output::nexts(ctx, &next);
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

/// `POST /v1/projects/{id}/imports`, and the name it got.
async fn import(ctx: &Context, at: Option<&PathBuf>, args: ImportFile) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let calcifer = ctx.calcifer().await?;
    let cwd = std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let hash = args.source.strip_prefix("sha256:").unwrap_or(&args.source);
    let by_hash = hash.len() >= 6 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) && !cwd.join(&args.source).exists();
    let source = match args.source.contains("://") || by_hash {
        true => args.source,
        false => protocol::target::spelled(&args.source, &cwd, &ctx.user_home).to_string_lossy().into_owned(),
    };
    if let Some(box_name) = args.into {
        let joined: VersionMoved = calcifer.post(&crate::calcifer_client::path(&["v1", "projects", &opened.id, "assets", &box_name, "versions"]), &NewVersion { source }, None).await?;
        if ctx.json {
            output::json(&joined);
        } else {
            anstream::println!("{box_name}.{}", joined.number);
        }
        return Ok(ExitCode::SUCCESS);
    }
    let path = format!("/v1/projects/{}/imports", opened.id);
    let exact = args.name.is_some();
    let ask = Import { source, name: args.name, at: None, description: args.description, exact };
    let imported: Imported = calcifer.post(&path, &ask, Some(&mint_key())).await?;
    if ctx.json {
        output::json(&imported);
        return Ok(ExitCode::SUCCESS);
    }
    anstream::println!("{}", imported.name);
    Ok(ExitCode::SUCCESS)
}

/// `cozy project upgrade`: here, in this process, as the files are all it takes and a Calcifer that reads this format can't read them yet.
fn upgrade(ctx: &Context, at: Option<&PathBuf>, status: Option<&PathBuf>) -> Result<ExitCode, Error> {
    let dir = match at {
        Some(at) => at.clone(),
        None => std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?,
    };
    let said = status.map(|file| std::fs::read_to_string(file).map_err(|e| Error::new(Kind::Usage, "status_unreadable", format!("{}: {e}", file.display())))).transpose()?;
    let done = calcifer::project::upgrade(&dir, said.as_deref())?;
    if ctx.json {
        output::json(&done);
    } else {
        let table = output::Table::new(ctx, ["NAME", "WAS", "BEFORE", "AFTER", "VERSION"]);
        let rows = done.assets.iter().map(|row| [row.name.clone(), row.was.clone(), row.before.clone(), row.after.clone(), row.version.map_or("-".to_owned(), |number| number.to_string())]);
        rows.fold(table, output::Table::row).print();
        done.said.iter().for_each(|line| output::warn(line));
        let worse = done.assets.iter().filter(|row| row.before == "current" && row.after != "current").count();
        output::say(ctx, &format!("{} assets are format 3, and {worse} went out of date. The old files are project.yaml.v2 and project.lock.v2", done.assets.len()));
    }
    let worse = done.assets.iter().any(|row| row.before == "current" && row.after != "current");
    Ok(if worse { ExitCode::from(3) } else { ExitCode::SUCCESS })
}

async fn render(ctx: &Context, at: Option<&PathBuf>, args: Render) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let calcifer = ctx.calcifer().await?;
    let queued = args.queue;
    let args_plain = args.plain;
    // Opened before the ask, so nothing it sets going is missed.
    let mut events = match queued {
        true => None,
        false => Some(calcifer.watch::<Event>(&format!("/v1/projects/{}/events", opened.id)).await?),
    };
    // Named, it's held to what this OS can do. Unnamed it's `local`, and Calcifer says so only of what needs a machine: an Edit needs none.
    let on: Vec<On> = match args.on.is_empty() {
        true => vec![On::from("local".to_owned())],
        false => args.on.into_iter().map(|on| local_usable(&on).map(|()| On::from(on))).collect::<Result<_, _>>()?,
    };
    let runs = Runs {
        outdated: args.outdated,
        names: args.names,
        n: args.n,
        on,
        hub: ctx.hub().to_string(),
        approve: args.approve,
        missing: args.missing,
        take: args.take,
        in_s: args.in_s,
    };
    let path = format!("/v1/projects/{}/runs", opened.id);
    let asked: Asked = calcifer.post(&path, &runs, Some(&mint_key())).await?;
    match asked.skipped.as_slice() {
        [] => {}
        [one] => output::say(ctx, &format!("{one} is approved: skipped. Name it to make a new version, or unapprove it")),
        many => output::say(ctx, &format!("{} are approved: skipped. Name them to make new versions, or unapprove them", many.join(", "))),
    }
    if events.is_none() {
        if ctx.json {
            output::json(&asked);
            return Ok(ExitCode::SUCCESS);
        }
        for item in &asked.queued {
            match item.ahead.as_slice() {
                [] => output::say(ctx, &format!("queued {}, next", item.name)),
                ahead => output::say(ctx, &format!("queued {}, after {}", item.name, ahead.join(", "))),
            }
        }
        if asked.queued.is_empty() {
            output::say(ctx, "nothing to queue: what that names has its state, or is going already");
        }
        output::nexts(ctx, &asked.next);
        return Ok(ExitCode::SUCCESS);
    }
    if asked.joined {
        let name = opened.path.file_name().map_or_else(|| opened.path.display().to_string(), |name| name.to_string_lossy().into_owned());
        output::say(ctx, &match asked.added {
            0 => format!("a render is already going in {name}, and this adds nothing to it: joining it"),
            _ => format!("a render is already going in {name}: joining it"),
        });
    }
    let events = events.take().expect("following");
    let status = format!("cozy project -p {} status", opened.path.display());
    if ctx.json {
        return following_json(ctx, events, &status).await;
    }
    render::follow(ctx, render::Rendering::new(opened.path.clone(), asked.board), events, args_plain, &status).await
}

/// `--json`: each run's own events, as `cozy run` gives them, and the render's refusal as its envelope.
async fn following_json(ctx: &Context, mut events: crate::calcifer_client::Watch<Event>, status: &str) -> Result<ExitCode, Error> {
    let calcifer = ctx.calcifer().await?;
    let mut warned = std::collections::HashSet::new();
    // `/events` opens with the runs going, and one that began just then can be told twice.
    let mut followed = std::collections::HashSet::new();
    while let Some(event) = events.next().await {
        match event {
            Event::Started { run, .. } if followed.insert(run.clone()) => {
                let run = watched(ctx, calcifer.run(&run).await?).await?;
                if !settled(&run) {
                    return Ok(exit_code(&run));
                }
            }
            Event::Warning { message, .. } => {
                if warned.insert(message.clone()) {
                    output::warn(&message);
                }
            }
            Event::Ended { error, .. } => {
                return match error {
                    Some(error) => Err(error),
                    None => Ok(ExitCode::SUCCESS),
                };
            }
            _ => {}
        }
    }
    Err(Error::new(Kind::Unavailable, "project_events_ended", "Calcifer stopped telling how the runs go").next(status))
}

/// What `--json` keeps of a project (`--full`: all of it): what the view shows, by the record's own names.
pub(crate) const FIELDS: &[&str] = &[
    "id", "path", "busy", "moving", "held", "absent", "missing", "stale", "queue", "warnings", "next", "assets.name", "assets.kind",
    "assets.tool", "assets.standing", "assets.versions", "assets.version", "assets.approved", "assets.pinned", "assets.making",
    "assets.status", "assets.failure",
];

/// `cozy project status`: an asset a row, what makes it and how it stands, the totals, then what to know and do. `--full`:
/// every column and line, each file's path among them.
async fn status(ctx: &Context, at: Option<&PathBuf>) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let state: State = ctx.calcifer().await?.get(&format!("/v1/projects/{}", opened.id)).await?;
    shown(ctx, &state);
    Ok(ExitCode::SUCCESS)
}

/// `cozy project wait`: the render going, followed as `render` follows its own. None going: the project as `status` shows
/// it, failing when an asset's last try did.
async fn wait(ctx: &Context, at: Option<&PathBuf>) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let calcifer = ctx.calcifer().await?;
    // Opened before the read, so nothing between the two is missed.
    let events = calcifer.watch::<Event>(&format!("/v1/projects/{}/events", opened.id)).await?;
    let state: State = calcifer.get(&format!("/v1/projects/{}", opened.id)).await?;
    let status = format!("cozy project -p {} status", opened.path.display());
    match state.board.clone().filter(|_| state.busy) {
        Some(_) if ctx.json => following_json(ctx, events, &status).await,
        Some(board) => render::follow(ctx, render::Rendering::new(opened.path.clone(), board), events, false, &status).await,
        None => {
            shown(ctx, &state);
            let failed = state.assets.iter().any(|asset| asset.failure.is_some());
            Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
        }
    }
}

/// The project as `status` prints it.
fn shown(ctx: &Context, state: &State) {
    if ctx.json {
        return output::view(ctx, state, FIELDS);
    }
    state.warnings.iter().for_each(|warning| println!("warning: {}", warning.message));
    if ctx.full {
        let table = output::Table::new(ctx, ["NAME", "KIND", "MADE", "APPROVED", "MADE BY", "STATUS", "FILE"]);
        let rows = state.assets.iter().map(|a| {
            let made = match a.versions {
                1 => format!("1 {}", a.noun),
                count => format!("{count} {}s", a.noun),
            };
            let pinned = match a.approved {
                Some(number) => number.to_string(),
                None if a.pinned => "file".into(),
                None => "-".into(),
            };
            let made_by = a.made_by.clone().unwrap_or_else(|| "-".into());
            let file = a.path.clone().unwrap_or_else(|| "-".into());
            [a.name.clone(), a.kind.clone(), made, pinned, made_by, a.status.clone(), file]
        });
        rows.fold(table, output::Table::row).print();
    } else if !state.assets.is_empty() {
        let rows = state.assets.iter().map(|a| [a.name.clone(), a.tool.clone().unwrap_or_else(|| "file".into()), a.status.clone()]);
        rows.fold(output::Table::new(ctx, ["NAME", "TOOL", "STATE"]), output::Table::row).print();
    }
    println!("{}", totals(state));
    summary(ctx, state);
}

/// `4 assets: 1 approved, 1 current, 1 out of date, 1 missing`, leaving out what none are.
pub(crate) fn totals(state: &State) -> String {
    let standing = |asset: &protocol::project::Asset| match asset.standing {
        Standing::Making => "rendering",
        Standing::Missing => "missing",
        Standing::OutOfDate => "out of date",
        Standing::Current if asset.approved.is_some() || asset.pinned => "approved",
        Standing::Current => "current",
    };
    let said: Vec<String> = ["approved", "current", "out of date", "rendering", "missing"]
        .into_iter()
        .map(|w| (state.assets.iter().filter(|asset| standing(asset) == w).count(), w))
        .filter(|(n, _)| *n > 0)
        .map(|(n, w)| format!("{n} {w}"))
        .collect();
    match state.assets.len() {
        0 => "assets: none in project.yaml".into(),
        n => format!("{n} assets: {}", said.join(", ")),
    }
}

/// Under the table, on stdout: what's going, what failed, and what to do; with `--full`, each version's hash, what
/// is missing and out of date and why, what the file no longer names, and where the cuts are.
fn summary(ctx: &Context, state: &State) {
    if let Some(held) = &state.held {
        println!("{}", render::held_line(held));
    }
    let (running, queued): (Vec<_>, Vec<_>) = state.queue.iter().partition(|item| item.running);
    if !running.is_empty() {
        let shown: Vec<String> = running.iter().map(|item| item.run.map_or_else(|| item.name.clone(), |run| format!("{} (run {run})", item.name))).collect();
        println!("running: {}", shown.join(", "));
    }
    if !queued.is_empty() {
        let shown: Vec<&str> = queued.iter().map(|item| item.name.as_str()).collect();
        println!("queued: {}", shown.join(", "));
    }
    if !state.queue.is_empty() && !state.busy {
        println!("nothing is walking these: any `cozy project render` takes them up again");
    }
    for asset in &state.assets {
        if let Some(failure) = &asset.failure {
            output::failure_of(ctx, failure);
        }
    }
    if state.absent > 0 {
        println!("{} files project.lock names are not on this computer", state.absent);
    }
    if ctx.full {
        full_summary(ctx, state);
    } else {
        for cut in state.cuts.iter().filter(|cut| !cut.without.is_empty()) {
            println!("{}{}", cut.cut.trim_end_matches(".mp4"), went_without(&cut.without));
        }
    }
    output::nexts(ctx, &state.next);
}

/// What `--full` adds under the table.
fn full_summary(ctx: &Context, state: &State) {
    for line in state.board.iter().flat_map(|board| &board.ahead).filter_map(render::ahead_line) {
        println!("{line}");
    }
    for asset in state.assets.iter().filter(|asset| !asset.prefixes.is_empty()) {
        let from = |made: &protocol::project::VersionPrefix| match made.from.as_slice() {
            [] => String::new(),
            pieces => format!(" (from {})", pieces.join(", ")),
        };
        let made: Vec<String> = asset.prefixes.iter().map(|made| format!("{} {} {}{}{}", asset.noun, made.number, made.prefix, from(made), went_without(&made.without))).collect();
        println!("{}: {}", asset.name, made.join(", "));
        for made in &asset.prefixes {
            if let Some(kept) = made.kept {
                println!("{}", kept.said(&asset.name, made.number));
            }
        }
    }
    if state.run_files > 0 {
        println!("{} files from `cozy run` here, belonging to no asset: approve one, or `cozy project reclaim` once nothing names them", state.run_files);
    }
    if !state.missing.is_empty() {
        println!("missing: {}", state.missing.join(", "));
    }
    if !state.stale.is_empty() {
        let stale: Vec<String> = state.stale.iter().map(|s| format!("{} ({})", s.name, s.why)).collect();
        println!("out of date: {}", stale.join(", "));
    }
    for orphan in &state.orphans {
        let (n, name, kind) = (orphan.versions, &orphan.name, &orphan.kind);
        println!("project.lock keeps {n} made of {name} (a {kind}), {}, which project.yaml no longer names", output::bytes(orphan.bytes));
    }
    for cut in &state.cuts {
        let skips = match (cut.without.is_empty(), cut.skips.as_slice()) {
            (false, _) => went_without(&cut.without),
            (true, []) => String::new(),
            (true, skips) => format!("; it would skip {} (nothing made yet)", skips.join(", ")),
        };
        let name = cut.cut.trim_end_matches(".mp4");
        let made = cut.frames.map(|frames| format!(", {}", length(frames, cut.fps))).unwrap_or_default();
        let retimed = retimes(&cut.retimed, cut.fps);
        match cut.cut_at_ms {
            Some(at_ms) => println!("{name}: {}{made}, cut {} ago{retimed}{skips}", output::link(ctx, &cut.path), output::age(at_ms)),
            None => println!("{name}: not cut yet ({})", cut.path.display()),
        }
    }
}

/// ` (without segment_3: no version yet)`, when a film went without something: a short film can't pass unnoticed.
fn went_without(pieces: &[String]) -> String {
    match pieces {
        [] => String::new(),
        pieces => format!(" (without {})", pieces.join("; ")),
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
pub(crate) async fn open(ctx: &Context, at: Option<&PathBuf>, create: bool) -> Result<Opened, Error> {
    let cwd = std::env::current_dir()
        .map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let path = match at {
        Some(at) => protocol::target::spelled(&at.to_string_lossy(), &cwd, &ctx.user_home),
        None => cwd,
    };
    let opened: Opened = ctx.calcifer().await?.post("/v1/projects", &Open { path: Some(path), create, name: None }, None).await?;
    opened.said.iter().for_each(|said| output::warn(said));
    Ok(opened)
}
