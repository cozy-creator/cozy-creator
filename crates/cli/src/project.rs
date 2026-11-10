//! Asset commands: the CLI's side of Calcifer's projects (xs-151, cl-758). Calcifer finds the
//! project, makes its assets' takes and versions and keeps their files; these verbs ask it and print,
//! and `render` draws what Calcifer says of it (`render.rs`). Ctrl-C stops following, never the runs.
mod assets;
pub(crate) mod render;

use std::{path::PathBuf, process::ExitCode};

use api::{
    calcifer::{MachineList, On, local_usable},
    error::{Error, Kind},
    calcifer::hub::At,
    calcifer::project::{
        Approve, Approved, Asked, By, Cancel, Cancelled, Event, Fit, FitAsk, History, HistoryStep, HistoryStepped, Import, Imported, NewVersion, Open, Opened, Pickup, Reclaim,
        Reclaimed, Rename, Renamed, Runs, Standing, State, Transferred, Unpin, Unpinned, VersionMoved,
    },
};

use crate::{
    Context, output,
    run::{exit_code, mint_key, settled, watched},
};

/// A push or pull that moved all it could and left out a file.
const PARTIAL: u8 = 3;

const RENDER_EXAMPLES: &str = "Examples:
  cozy render all --on kiyui               a new version of everything not approved, each reading the new ones before it
  cozy render all --on kiyui,jitae         spread over two machines
  cozy render all --missing --on kiyui     only what has no state: continues a render that half failed
  cozy render fight --on kiyui             a new take of fight, and of what it reads only if that has no state
  cozy render fight -n 3 --on kiyui        three new takes of fight; approved, what it approved stays
  cozy render fight --approve --on kiyui   a new take, approved as it lands
  cozy render all --outdated --on kiyui    and of each approved asset whose inputs changed, as its turn comes: the new one takes its place
  cozy render fight pickup.take='<fight.2>' pickup.out_frame=240
                                                   Pick up from here: take 2 up to frame 240, the rest new
  cozy render shot_2 pickup.take='<shot_1.3>' pickup.out_frame=120 pickup.beats_from=7
                                                   picking up another box's take, its beats from 7 s

An asset's state is the hash in its image:, video: or audio: (`approve` writes that line, `unapprove` removes it), else its newest
version with these inputs, else none. Reverting a text returns the old output with no run. Every render makes new versions, except of what is approved and not named.

Ctrl-C stops following, never the runs.";

/// Operations on the assets in the current directory's project.
#[derive(clap::Subcommand, Debug)]
pub(crate) enum Verb {
    Import(ImportFile),
    Add(assets::Add),
    Set(assets::Set),
    /// Delete assets, as the X on a box: their work stops, and what they made stays until `reclaim`. `<name>.<n>` discards that take
    /// or version instead. `cozy undo` puts either back, and each prints the command.
    #[command(after_help = "Examples:\n  cozy delete whale\n  cozy delete whale.2 whale.3   takes 2 and 3 of whale: off it, their numbers never used again\n  cozy undo 42                  the line it printed")]
    Delete {
        /// The assets, or `<name>.<n>` for one take or version of one.
        #[arg(required = true, value_name = "NAME")]
        names: Vec<String>,
    },
    Render(Render),
    /// Approve a take or version: its hash is the asset's `approved`.
    #[command(after_help = "Examples:\n  cozy approve fight 2\n  cozy approve kyogre 65b436d4f9a3   any file of the project's, by hash\n\nIt sets fight's `approved`, changing nothing else. `cozy unapprove fight` takes it away.")]
    Approve {
        /// The asset: `theo`, `dock_b`, `fight`, `fight_upscale`, …
        name: String,
        /// Its take or version, 1 first, or a hash prefix of any file in the project's `files/`.
        version: String,
    },
    /// Drop assets' queued work and cancel their runs, or `all`.
    #[command(after_help = "Examples:\n  cozy cancel clash\n  cozy cancel all")]
    Cancel {
        /// The assets, or `all`.
        #[arg(required = true, value_name = "NAME")]
        names: Vec<String>,
    },
    /// Take back an asset's approval: its `approved` goes, and its state follows its inputs again.
    #[command(after_help = "Example:\n  cozy unapprove fight")]
    Unapprove {
        /// The asset.
        name: String,
    },
    /// What has no state and why, and each asset's file; with a name, that asset's fields and each take or version.
    #[command(alias = "ls", after_help = "Examples:\n  cozy status\n  cozy status fight")]
    Status {
        /// One asset.
        name: Option<String>,
    },
    /// Follow the render going to its end, as `render` follows its own: free, it starts nothing. With none going, the project as it stands.
    #[command(after_help = "Example:\n  cozy render clash --on corteo --queue\n  cozy wait\n\nExits 0 done, 1 something failed, 4 a run stopped moving: it goes on until you cancel it. Ctrl-C stops waiting, never the render.")]
    Wait,
    /// What was done to the project, newest first: each edit by the page, the CLI or an import, and whether it's undone.
    #[command(after_help = "Example:\n  cozy history\n  cozy undo 42   that entry, if nothing since changed what it did")]
    History {
        /// How many, newest first.
        #[arg(short = 'n', value_name = "N", default_value_t = 20)]
        limit: usize,
    },
    /// Undo the project's newest edit, whoever made it: the page, the CLI, or an import. Refused if a later edit changed the same thing.
    /// Renders aren't undone, and nothing is rented or ended.
    #[command(after_help = "Examples:\n  cozy undo\n  cozy undo 42        that entry of `cozy history`\n  cozy undo --to 40   every edit back to 40")]
    Undo {
        /// The entry, from `cozy history`.
        #[arg(value_name = "ID")]
        entry: Option<u64>,
        /// Every edit back to this one.
        #[arg(long, value_name = "ID", conflicts_with = "entry")]
        to: Option<u64>,
    },
    /// Do again what undo last took back, unless an edit came since; with an ID, that entry.
    #[command(after_help = "Examples:\n  cozy redo\n  cozy redo 42\n  cozy redo --to 45   every edit undone, up to 45")]
    Redo {
        #[arg(value_name = "ID")]
        entry: Option<u64>,
        #[arg(long, value_name = "ID", conflicts_with = "entry")]
        to: Option<u64>,
    },
    /// Rename an asset: its key, every <mention> of it, its versions and its place on the canvas.
    #[command(after_help = "Example:\n  cozy rename theo mara")]
    Rename {
        /// Its name now, or an orphan's whose versions the project still keeps.
        from: String,
        /// Its new name.
        to: String,
    },
    /// List what the project no longer names, and with --yes delete it: its versions, and any file nothing else names.
    #[command(after_help = "Example:\n  cozy reclaim\n  cozy reclaim --yes\n\nNothing is ever deleted without it.")]
    Reclaim {
        /// Delete them.
        #[arg(long)]
        yes: bool,
    },
    /// Send every file the project names that your Tensorhub account lacks, private to it.
    #[command(after_help = "Example:\n  cozy push\n\nA file over 5 GiB goes as parts, which the project lists. A file on neither this computer nor the Hub is skipped with a warning, everything else goes, and it exits 3.\nCtrl-C stops following, never the transfer.")]
    Push,
    /// Fetch every file the project names that files/ lacks, as real files.
    #[command(after_help = "Example:\n  git clone … && cd garden && cozy pull\n\nA file not at the Hub is skipped with a warning, everything else comes, and it exits 3.\nCtrl-C stops following, never the transfer.")]
    Pull,
    /// The project as YAML, format 4: for reading, a diff or git. `cozy import <file>.yaml` reads it back.
    #[command(after_help = "Examples:\n  cozy export\n  cozy export -o garden.yaml && $EDITOR garden.yaml && cozy import garden.yaml\n\nIt holds what the project says, its boxes and fields; its versions, takes and files stay in project.db.")]
    Export {
        /// Write it to this file, not stdout.
        #[arg(short = 'o', long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
}

/// Add a picture, video or sound to the project as an element with a name, and print the name it got. A `.yaml` file is the
/// project as `export` writes it, read back: what differs is written, in one step `undo` takes back.
#[derive(clap::Args, Debug)]
#[command(after_help = "Examples:
  cozy import garden.yaml                the project as that file says, each box that differs
  cozy import ./kyogre.png               prints kyogre
  cozy import ./kyogre.png kyogre_art    prints kyogre_art
  cozy import https://example.com/groudon.png
  cozy import 65b436d4f9a3 kyogre --description 'The colossal blue whale.'    a file of the project's, as an asset of its own
  cozy import ./kyogre2.png --into kyogre    the next version of kyogre: <kyogre.2>

A name something in the project has takes _2, and the same file twice is two elements and one file: <name> reads it in any text.
A URL is downloaded here, up to 64 MiB, from a public address.")]
pub(crate) struct ImportFile {
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
pub(crate) struct Render {
    /// The assets (`theo`, `dock_b`, `fight`, `fight_upscale`, …), with what they read, or `all`: every asset. For one box, `pickup.take=<box.N>`,
    /// `pickup.out_frame=<frame>` and `pickup.beats_from=<seconds>` pick up from that take.
    #[arg(required = true, value_name = "NAME")]
    names: Vec<String>,
    /// The machines the renders go to: `local`, machine names, or `any`, comma-separated. None: your rentals and this computer, each render
    /// on whichever ends it first, this computer last.
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
}


pub(crate) async fn exec(ctx: &Context, verb: Verb) -> Result<ExitCode, Error> {
    let project = ensure(ctx, None).await?;
    match verb {
        Verb::Import(import) => return self::import(ctx, Some(&project), import).await,
        Verb::Export { out } => return export(ctx, Some(&project), out).await,
        Verb::Add(add) => return assets::add(ctx, Some(&project), add).await,
        Verb::Set(set) => return assets::set(ctx, Some(&project), set).await,
        Verb::Delete { names } => return assets::delete(ctx, Some(&project), names).await,
        Verb::Render(render) => return self::render(ctx, Some(&project), render).await,
        Verb::Approve { name, version } => {
            let path = at(&project, &["approve"]);
            let approved: Approved = ctx.calcifer().await?.post(&path, &Approve { name: name.clone(), version }, None).await?;
            approved.warnings.iter().for_each(|warning| output::warn(warning));
            if ctx.json {
                output::json(&approved);
            } else {
                let (name, noun, number, path) = (&approved.name, &approved.noun, approved.number, &approved.path);
                match number {
                    0 => output::say(ctx, &format!("{name}: approved ({path})")),
                    number => output::say(ctx, &format!("{name}: {noun} {number} approved ({path})")),
                }
                if !approved.stale.is_empty() {
                    output::say(ctx, &format!("out of date now: {}", approved.stale.join(", ")));
                }
            }
        }
        Verb::Unapprove { name } => {
            let path = at(&project, &["unpin"]);
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
            let path = at(&project, &["cancel"]);
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
            let path = at(&project, &["reclaim"]);
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
            if !reclaimed.unheld.is_empty() {
                let left = reclaimed.unheld.iter().map(|left| match &left.sha256 {
                    Some(sha256) => format!("{} ({})", left.path, &sha256[..sha256.len().min(19)]),
                    None => format!("{}/", left.path),
                });
                let said = format!("files/ doesn't hold what runs left here, so it stays: {}", left.collect::<Vec<_>>().join(", "));
                output::say(ctx, &said);
            }
            if let Some(lost) = &reclaimed.lost {
                output::say(ctx, &format!("{lost}: --yes is refused"));
            }
            output::nexts(ctx, &reclaimed.next);
        }
        Verb::Status { name: Some(name) } => return assets::one(ctx, Some(&project), name).await,
        Verb::Status { name: None } => return status(ctx, Some(&project)).await,
        Verb::Wait => return wait(ctx, Some(&project)).await,
        Verb::Rename { from, to } => {
            let path = at(&project, &["rename"]);
            let renamed: Renamed = ctx.calcifer().await?.post(&path, &Rename { from, to }, None).await?;
            match ctx.json {
                true => output::json(&renamed),
                false if renamed.versions == 0 => output::say(ctx, &format!("{} is {} now", renamed.from, renamed.to)),
                false => output::say(ctx, &format!("{} is {} now: its {} kept", renamed.from, renamed.to, renamed.versions)),
            }
        }
        Verb::History { limit } => return history(ctx, Some(&project), limit).await,
        Verb::Undo { entry, to } => return step(ctx, Some(&project), "undo", HistoryStep { entry, to, edit: None }).await,
        Verb::Redo { entry, to } => return step(ctx, Some(&project), "redo", HistoryStep { entry, to, edit: None }).await,
        Verb::Push => {
            let done = transfer(ctx, Some(&project), "push", "sent").await?;
            let at = done.kept_in.as_deref().unwrap_or("the Hub");
            let (moved, bytes, rest) = (done.moved, output::bytes(done.bytes), rest(&done));
            output::say(ctx, &format!("{} files at {at}: {moved} sent ({bytes}), {rest} there already", done.files));
            return Ok(skipped(&done));
        }
        Verb::Pull => {
            let done = transfer(ctx, Some(&project), "pull", "fetched").await?;
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
        ("file", _) => "import".to_owned(),
        (via, Some(client)) => format!("{via} {client}"),
        (via, None) => via.to_owned(),
    }
}

/// `GET /v1/projects/{id}/history`.
async fn history(ctx: &Context, at: Option<&String>, limit: usize) -> Result<ExitCode, Error> {
    let listed: History = ctx.calcifer().await?.get_query(&self::at(&named(at)?, &["history"]), &[("limit", limit)]).await?;
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
    let next: Vec<String> = [listed.undo.map(|_| "cozy undo".to_owned()), listed.redo.map(|_| "cozy redo".to_owned())].into_iter().flatten().collect();
    output::nexts(ctx, &next);
    Ok(ExitCode::SUCCESS)
}

/// `POST /v1/projects/{id}/history/<verb>`, and what it did.
async fn step(ctx: &Context, at: Option<&String>, verb: &str, asked: HistoryStep) -> Result<ExitCode, Error> {
    let stepped: HistoryStepped = ctx.calcifer().await?.post(&self::at(&named(at)?, &["history", verb]), &asked, None).await?;
    if ctx.json {
        output::json(&stepped);
        return Ok(ExitCode::SUCCESS);
    }
    let done = if verb == "undo" { "undid" } else { "redid" };
    if stepped.already {
        output::say(ctx, &format!("nothing to {verb}: that's {done} already"));
    }
    for entry in &stepped.entries {
        // Its answer, on stdout, as every write's is.
        println!("{done} {}: {}", entry.id, entry.words);
    }
    if let Some(stopped) = &stepped.stopped {
        output::warn(&stopped.message);
        output::nexts(ctx, &stopped.next);
        return Ok(ExitCode::FAILURE);
    }
    let next: Vec<String> = match verb {
        "undo" => stepped.entries.last().map(|entry| format!("cozy redo {}", entry.id)).into_iter().collect(),
        _ => stepped.entries.last().map(|entry| format!("cozy undo {}", entry.id)).into_iter().collect(),
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
async fn transfer(ctx: &Context, at: Option<&String>, verb: &str, moved: &str) -> Result<Transferred, Error> {
    let project = named(at)?;
    let calcifer = ctx.calcifer().await?;
    let mut events = calcifer.watch::<Event>(&self::at(&project, &["events"])).await?;
    let ask = At { hub: Some(ctx.hub().to_string()) };
    let _: Opened = calcifer.post(&self::at(&project, &[verb]), &ask, None).await?;
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
    Err(Error::new(Kind::Unavailable, "project_events_ended", said).next(format!("cozy {verb}")))
}

/// `POST /v1/projects/{id}/imports`, and the name it got.
async fn import(ctx: &Context, at: Option<&String>, args: ImportFile) -> Result<ExitCode, Error> {
    let project = named(at)?;
    let calcifer = ctx.calcifer().await?;
    let cwd = std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    if is_yaml(&args.source) && args.into.is_none() {
        let file = cwd.join(&args.source);
        let text = std::fs::read_to_string(&file).map_err(|e| Error::new(Kind::Usage, "import_unreadable", format!("{}: {e}", file.display())))?;
        calcifer.put_text(&self::at(&project, &["yaml"]), &text).await?;
        if !ctx.json {
            output::say(ctx, &format!("imported {}", file.display()));
        }
        return Ok(ExitCode::SUCCESS);
    }
    let hash = args.source.strip_prefix("sha256:").unwrap_or(&args.source);
    let by_hash = hash.len() >= 6 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) && !cwd.join(&args.source).exists();
    let exact = args.name.is_some();
    // A file here goes by content, named for its stem unless a name was typed.
    let (source, name) = match args.source.contains("://") || by_hash {
        true => (args.source, args.name),
        false => {
            let file = api::target::spelled(&args.source, &cwd, &ctx.user_home);
            let stem = file.file_stem().map(|stem| stem.to_string_lossy().to_lowercase());
            let digest = crate::send::Sender::new(ctx).await?.file(&file).await.map_err(|e| Error { code: "import_unreadable".into(), ..e })?;
            (digest.to_string(), args.name.or(stem))
        }
    };
    if let Some(box_name) = args.into {
        let joined: VersionMoved = calcifer.post(&self::at(&project, &["assets", &box_name, "versions"]), &NewVersion { source }, None).await?;
        if ctx.json {
            output::json(&joined);
        } else {
            anstream::println!("{box_name}.{}", joined.number);
        }
        return Ok(ExitCode::SUCCESS);
    }
    let path = self::at(&project, &["imports"]);
    let ask = Import { source, name, at: None, description: args.description, exact, moves: Default::default(), placed: Default::default() };
    let imported: Imported = calcifer.post(&path, &ask, Some(&mint_key())).await?;
    if ctx.json {
        output::json(&imported);
        return Ok(ExitCode::SUCCESS);
    }
    anstream::println!("{}", imported.name);
    Ok(ExitCode::SUCCESS)
}

/// `GET /v1/projects/{id}/yaml`: the project as YAML, to stdout or `out`.
async fn export(ctx: &Context, at: Option<&String>, out: Option<PathBuf>) -> Result<ExitCode, Error> {
    let text = ctx.calcifer().await?.text(&self::at(&named(at)?, &["yaml"])).await?;
    match out {
        Some(out) => {
            std::fs::write(&out, &text).map_err(|e| Error::new(Kind::Unavailable, "export_write", format!("{}: {e}", out.display())))?;
            output::say(ctx, &format!("wrote {}", out.display()));
        }
        None => print!("{text}"),
    }
    Ok(ExitCode::SUCCESS)
}

/// Whether `source` names the project as YAML, read back, rather than a file to add to it.
fn is_yaml(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    !source.contains("://") && (lower.ends_with(".yaml") || lower.ends_with(".yml"))
}

/// `pickup.take=<shot_1.3>`, `pickup.out_frame=240` and `pickup.beats_from=7` out of `names`: Pick up from here.
fn pickup_of(names: &mut Vec<String>) -> Result<Option<Pickup>, Error> {
    let (terms, rest): (Vec<String>, Vec<String>) = std::mem::take(names).into_iter().partition(|name| name.starts_with("pickup."));
    *names = rest;
    if terms.is_empty() {
        return Ok(None);
    }
    let malformed = |why: String| Error::new(Kind::Usage, "pickup_malformed", why).remedy("pickup.take=<box.N> pickup.out_frame=<frame> [pickup.beats_from=<seconds>]");
    let mut said = serde_json::Map::new();
    for term in terms {
        let (key, value) = term["pickup.".len()..].split_once('=').ok_or_else(|| malformed(format!("{term} says no value")))?;
        let value = match key {
            "take" => serde_json::json!(value),
            "out_frame" | "beats_from" => serde_json::from_str(value).map_err(|_| malformed(format!("{term}: {value} is not a number")))?,
            _ => return Err(malformed(format!("a pickup has no {key}"))),
        };
        said.insert(key.to_owned(), value);
    }
    serde_json::from_value(serde_json::Value::Object(said)).map(Some).map_err(|why| malformed(why.to_string()))
}

async fn render(ctx: &Context, at: Option<&String>, mut args: Render) -> Result<ExitCode, Error> {
    let pickup = pickup_of(&mut args.names)?;
    let project = named(at)?;
    let calcifer = ctx.calcifer().await?;
    let queued = args.queue;
    // Opened before the ask, so nothing it sets going is missed.
    let mut events = match queued {
        true => None,
        false => Some(calcifer.watch::<Event>(&self::at(&project, &["events"])).await?),
    };
    // Named, it's held to what this OS can do. Unnamed it's every rental of yours and this computer last, as the page sends it, each job going
    // where it ends first; with none, `local`, and Calcifer says so only of what needs a machine: an Edit needs none.
    let on: Vec<On> = match args.on.is_empty() {
        true => calcifer.get::<MachineList>("/v1/machines").await.map_or_else(|_| vec![On::from("local".to_owned())], |list| list.render_set()),
        false => args.on.into_iter().map(|on| local_usable(&on).map(|()| On::from(on))).collect::<Result<_, _>>()?,
    };
    if matches!(on.as_slice(), [only] if only.is_local()) {
        poorly_here(&calcifer, &project, &args.names, args.outdated).await;
    }
    let runs = Runs {
        outdated: args.outdated,
        names: args.names,
        n: args.n,
        on,
        hub: ctx.hub().to_string(),
        approve: args.approve,
        missing: args.missing,
        pickup,
    };
    let path = self::at(&project, &["runs"]);
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
        let name = asked.path.file_name().map_or_else(|| project.clone(), |name| name.to_string_lossy().into_owned());
        output::say(ctx, &match asked.added {
            0 => format!("a render is already going in {name}, and this adds nothing to it: joining it"),
            _ => format!("a render is already going in {name}: joining it"),
        });
    }
    let events = events.take().expect("following");
    let status = "cozy status";
    if ctx.json {
        return following_json(ctx, events, status, asked.ask.as_deref()).await;
    }
    render::follow(ctx, render::Rendering::new(asked.path.clone(), asked.board, asked.ask), events, status).await
}

/// What this computer would render poorly, in the line the page asks with, said once as a warning: this never asks, and goes on. A
/// Calcifer that can't say says nothing.
async fn poorly_here(calcifer: &crate::calcifer_client::Client, project: &str, names: &[String], outdated: bool) {
    let asked = FitAsk { names: names.to_vec(), outdated };
    let Ok(fit) = calcifer.post::<_, Fit>(&at(project, &["fit"]), &asked, None).await else { return };
    if fit.tools.is_empty() {
        return;
    }
    let lines = fit.tools.iter().map(|tool| tool.line.as_str()).chain(fit.rent.as_ref().map(|rent| rent.line.as_str()));
    output::warn(&lines.collect::<Vec<_>>().join(" "));
}

/// `--json`: each run's own events, as `cozy run` gives them, and the render's refusal as its envelope. It ends with the walk that ends
/// `ask`, or with None the one going.
async fn following_json(ctx: &Context, mut events: crate::calcifer_client::Watch<Event>, status: &str, ask: Option<&str>) -> Result<ExitCode, Error> {
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
            Event::Ended { error, asks, .. } if ask.is_none_or(|ask| asks.iter().any(|ended| ended == ask)) => {
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

/// `cozy status`: an asset a row, what makes it and how it stands, the totals, then what to know and do. `--full`:
/// every column and line, each file's path among them.
async fn status(ctx: &Context, at: Option<&String>) -> Result<ExitCode, Error> {
    let state: State = ctx.calcifer().await?.get(&self::at(&named(at)?, &[])).await?;
    shown(ctx, &state);
    Ok(ExitCode::SUCCESS)
}

/// `cozy wait`: the render going, followed as `render` follows its own. None going: the project as `status` shows
/// it, failing when an asset's last try did.
async fn wait(ctx: &Context, at: Option<&String>) -> Result<ExitCode, Error> {
    let project = named(at)?;
    let calcifer = ctx.calcifer().await?;
    // Opened before the read, so nothing between the two is missed.
    let events = calcifer.watch::<Event>(&self::at(&project, &["events"])).await?;
    let state: State = calcifer.get(&self::at(&project, &[])).await?;
    let status = "cozy status";
    match state.board.clone().filter(|_| state.busy) {
        Some(_) if ctx.json => following_json(ctx, events, status, None).await,
        Some(board) => render::follow(ctx, render::Rendering::new(state.path.clone(), board, None), events, status).await,
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
    let standing = |asset: &api::calcifer::project::Asset| match asset.standing {
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
        println!("nothing is walking these: any `cozy render` takes them up again");
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
        let from = |made: &api::calcifer::project::VersionPrefix| match made.from.as_slice() {
            [] => String::new(),
            pieces => format!(" (from {})", pieces.join(", ")),
        };
        let made: Vec<String> = asset.prefixes.iter().map(|made| format!("{} {} {}{}{}", asset.noun, made.number, made.prefix, from(made), went_without(&made.without))).collect();
        println!("{}: {}", asset.name, made.join(", "));
        for made in &asset.prefixes {
            if let Some(pickup) = &made.pickup {
                println!("{}", pickup.said(&asset.name, made.number));
            }
        }
    }
    if state.run_files > 0 {
        println!("{} files from `cozy run` here, belonging to no asset: approve one, or `cozy reclaim` once nothing names them", state.run_files);
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

/// The explicit project a caller resolved, or the current directory.
pub(crate) fn named(at: Option<&String>) -> Result<String, Error> {
    if let Some(at) = at.filter(|at| !at.is_empty()) {
        return Ok(at.clone());
    }
    let cwd = std::env::current_dir()
        .map_err(|error| Error::new(Kind::Unavailable, "cwd", format!("the current directory: {error}")))?;
    cwd.into_os_string().into_string()
        .map_err(|_| Error::new(Kind::Usage, "project_path", "the current directory is not a UTF-8 path"))
}

/// Open the current directory's existing project, or create its empty project on first use.
pub(crate) async fn ensure(ctx: &Context, at: Option<&String>) -> Result<String, Error> {
    let project = named(at)?;
    let path = format!("{}?assets=false", self::at(&project, &[]));
    let calcifer = ctx.calcifer().await?;
    match calcifer.get::<serde_json::Value>(&path).await {
        Ok(_) => return Ok(project),
        Err(error) if error.code == "project_not_found" => {}
        Err(error) => return Err(error),
    }
    let ask = Open { path: Some(PathBuf::from(&project)), ..Open::default() };
    match calcifer.post::<_, Opened>("/v1/projects", &ask, Some(&mint_key())).await {
        Ok(opened) => opened.said.iter().for_each(|said| output::warn(said)),
        Err(error) if error.code == "project_exists" => {
            calcifer.get::<serde_json::Value>(&path).await?;
        }
        Err(error) => return Err(error),
    }
    Ok(project)
}

/// `/v1/projects/<project>[/<rest>…]`: a route takes a project's name, id or folder.
pub(crate) fn at(project: &str, rest: &[&str]) -> String {
    let segments: Vec<&str> = ["v1", "projects", project].into_iter().chain(rest.iter().copied()).collect();
    crate::calcifer_client::path(&segments)
}
