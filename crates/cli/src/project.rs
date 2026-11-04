//! `cozy project`: the CLI's side of Calcifer's projects (xs-151, cl-758). Calcifer finds the
//! project, makes its assets' takes and versions and keeps their files; these verbs ask it and print,
//! and `render` follows each run as `cozy run` does. Ctrl-C stops following, never the runs.
use std::{path::PathBuf, process::ExitCode};

use protocol::{
    calcifer::{On, local_usable},
    error::{Error, Kind},
    hub::At,
    project::{Approve, Approved, Event, Import, Imported, Open, Opened, Projects, Reclaim, Reclaimed, Rename, Renamed, Rendered, Runs, State, Transferred, Unpin, Unpinned},
};

use crate::{
    Context, output,
    run::{mint_key, settled, watched},
};

/// A push or pull that moved all it could and left out a file.
const PARTIAL: u8 = 3;

const EXAMPLES: &str = "Examples:
  cozy project new garden
  cd garden && cozy project import ./kyogre.png   prints what to paste into project.yaml
  cozy project render all --on kiyui
  cozy project render fight --new -n 3 --on kiyui    three more takes of fight
  cozy project approve fight 2                 writes its hash into fight's lines in project.yaml: fight is a constant now
  cozy project unpin fight                     removes that line
  cozy project rename theo mara                after renaming it in project.yaml
  cozy project reclaim --yes                   deletes what project.yaml no longer names
  cozy project push                            its files to your Tensorhub account
  cozy project pull                            on another computer, beside project.yaml and project.lock

project.yaml's comments and the README explain the rest.";

const RENDER_EXAMPLES: &str = "Examples:
  cozy project render all --on kiyui               everything with no state; the cuts follow, free
  cozy project render all --on kiyui,jitae         spread over two machines
  cozy project render fight --on kiyui             fight and what it reads, if it has no state
  cozy project render fight --new --on kiyui       a new take of fight, whatever stands
  cozy project render fight --new -n 3 --on kiyui  three new takes of fight
  cozy project render fight --new --approve --on kiyui
  cozy project render all --new --on kiyui         every asset with an operation again
  cozy project render fight --in 15 --on kiyui     a new take: the take in use up to about 15 s, the rest new
  cozy project render fight --take 2 --in 15 --on kiyui   the same, keeping take 2
  cozy project render fight --take 2 --on kiyui    a new take: take 2, continued from its end

An asset's state is the hash in its image:, video: or audio: (`approve` writes that line, `unpin` removes it), else its newest
version with these inputs, else none. Reverting a text returns the old output with no run.

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
    Render(Render),
    /// Pin a take or version: its hash goes on the asset's line in project.yaml.
    #[command(after_help = "Examples:\n  cozy project approve fight 2\n  cozy project approve kyogre 65b436d4f9a3   any file of the project's, by hash\n\nIt writes `video: <hash>` (or `image:`) into fight in project.yaml, changing nothing else. `cozy project unpin fight` removes it.")]
    Approve {
        /// The asset: `theo`, `dock_b`, `fight`, `fight_upscale`, …
        name: String,
        /// Its take or version, 1 first, or a hash prefix of any file in the project's `files/`.
        version: String,
    },
    /// Take an asset's pin out of project.yaml: its state follows its inputs again.
    #[command(after_help = "Example:\n  cozy project unpin fight")]
    Unpin {
        /// The asset.
        name: String,
    },
    /// What has no state and why, and each asset's file.
    #[command(after_help = "Example:\n  cozy project status")]
    Status,
    /// Keep an asset's versions after renaming it in project.yaml.
    #[command(after_help = "Example:\n  cozy project rename theo mara   (status suggests it after a rename)")]
    Rename {
        /// Its name in project.lock.
        from: String,
        /// Its name in project.yaml now: an asset with no versions.
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
}

/// Bring a picture, video or sound into the project, and print the line that names it in project.yaml.
#[derive(clap::Args, Debug)]
#[command(after_help = "Examples:
  cozy project import ./kyogre.png
  cozy project import https://example.com/groudon.png
  cozy project import ./coast.jpg

A URL is downloaded here, up to 64 MiB, from a public address.")]
struct ImportFile {
    /// A path, or an http(s) URL.
    source: String,
    /// Remove a picture's background, by a run of `birefnet/remove_background`, and keep only the result, printing its line. Without
    /// this, import is a plain copy and needs no machine.
    #[arg(long)]
    remove_background: bool,
    /// The machine that run goes to (default: this computer if it can run models, else a machine you have up).
    #[arg(long = "on", value_name = "MACHINE", requires = "remove_background")]
    on: Option<String>,
}

/// For each asset named and what it reads, if it has no state, run its operation. Nothing else runs.
#[derive(clap::Args, Debug)]
#[command(after_help = RENDER_EXAMPLES)]
struct Render {
    /// The assets (`theo`, `dock_b`, `fight`, `fight_upscale`, …), with what they read, or `all`: every asset.
    #[arg(required = true, value_name = "NAME")]
    names: Vec<String>,
    /// The machines the renders go to: `local`, machine names, or `any`, comma-separated.
    #[arg(long = "on", value_name = "MACHINE", value_delimiter = ',', default_value = "local")]
    on: Vec<String>,
    /// How many new takes or versions of each asset named: over 1 draws new seeds as --new does.
    #[arg(short = 'n', value_name = "N", default_value_t = 1)]
    n: u32,
    /// A new seed even with a state. A pinned asset's new one is to look at: its state doesn't move.
    #[arg(long)]
    new: bool,
    /// Pin the new take or version when it lands (only with -n 1).
    #[arg(long)]
    approve: bool,
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
            // A short run of digits is a version; anything else, a file's hash.
            let number = version.parse::<u32>().ok().filter(|_| version.len() <= 6);
            let file = number.is_none().then_some(version);
            let approved: Approved =
                ctx.calcifer().await?.post(&path, &Approve { name: name.clone(), version: number, file }, None).await?;
            approved.warnings.iter().for_each(|warning| output::warn(warning));
            if ctx.json {
                output::json(&approved);
            } else {
                let (name, noun, number) = (&approved.name, &approved.noun, approved.number);
                let path = output::link(ctx, &opened.path.join(&approved.path));
                match number {
                    0 => output::say(ctx, &format!("{name}: pinned in project.yaml ({path})")),
                    number => output::say(ctx, &format!("{name}: {noun} {number} pinned in project.yaml ({path})")),
                }
                if !approved.stale.is_empty() {
                    output::say(ctx, &format!("out of date now: {}", approved.stale.join(", ")));
                }
            }
        }
        Verb::Unpin { name } => {
            let opened = open(ctx, args.path.as_ref(), false).await?;
            let path = format!("/v1/projects/{}/unpin", opened.id);
            let unpinned: Unpinned = ctx.calcifer().await?.post(&path, &Unpin { name }, None).await?;
            if ctx.json {
                output::json(&unpinned);
            } else {
                match unpinned.version {
                    Some(version) => output::say(ctx, &format!("{}: no longer pinned to {version}", unpinned.name)),
                    None => output::say(ctx, &format!("{}: no longer pinned", unpinned.name)),
                }
                if !unpinned.stale.is_empty() {
                    output::say(ctx, &format!("out of date now: {}", unpinned.stale.join(", ")));
                }
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
                (false, _) => {
                    output::say(ctx, &format!("{files} files nothing else names, {bytes}"));
                    output::next(ctx, &["cozy project reclaim --yes"]);
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
            if !state.entities.is_empty() {
                let table = output::Table::new(ctx, ["ENTITY", "KIND", "LINKS"]);
                let linked = |entity: &protocol::project::Entity| {
                    entity.links.iter().map(|(field, asset)| format!("{field}: {asset}")).collect::<Vec<_>>().join(", ")
                };
                let rows = state.entities.iter().flat_map(|entity| std::iter::once(entity).chain(&entity.subs));
                rows.fold(table, |table, entity| table.row([entity.name.clone(), entity.kind.clone(), linked(entity)])).print();
            }
            let table = output::Table::new(ctx, ["NAME", "KIND", "MADE", "PINNED", "MADE BY", "STATUS", "FILE"]);
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

/// `POST /v1/projects/{id}/imports`, and what to paste.
async fn import(ctx: &Context, at: Option<&PathBuf>, args: ImportFile) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let calcifer = ctx.calcifer().await?;
    let cwd = std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let source = match args.source.contains("://") {
        true => args.source,
        false => protocol::target::spelled(&args.source, &cwd, &ctx.user_home).to_string_lossy().into_owned(),
    };
    let (remove_background, on) = (args.remove_background, args.on.map(On::from));
    let imported: Imported = calcifer.post(&format!("/v1/projects/{}/imports", opened.id), &Import { source, remove_background, on }, None).await?;
    if ctx.json {
        output::json(&imported);
        return Ok(ExitCode::SUCCESS);
    }
    let verb = if imported.already { "already imported" } else { "imported" };
    if !remove_background {
        anstream::println!("{verb} {}", imported.prefix);
        anstream::println!("{}", imported.line);
        return Ok(ExitCode::SUCCESS);
    }
    // One line on stdout, the one to paste; the rest goes where a run's progress goes.
    if let Some(note) = &imported.note {
        output::warn(note);
    }
    let Some(run) = imported.run else {
        anstream::println!("{}", imported.line);
        return Ok(ExitCode::SUCCESS);
    };
    let run = crate::run::follow(ctx, *run).await?;
    if run.status == protocol::calcifer::Status::Completed {
        crate::run::paste(ctx, &run);
        return Ok(ExitCode::SUCCESS);
    }
    output::warn(&format!("run {} hasn't removed its background, and nothing is kept: import it again", run.number));
    Ok(crate::run::exit_code(&run))
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
        approve: args.approve,
        new: args.new,
        take: args.take,
        in_s: args.in_s,
    };
    let path = format!("/v1/projects/{}/runs", opened.id);
    let _: Opened = calcifer.post(&path, &runs, Some(&mint_key())).await?;
    let status = format!("cozy project -p {} status", opened.path.display());
    let (mut failed, mut warned) = (std::collections::HashSet::new(), std::collections::HashSet::new());
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
            Event::Warning { message } => {
                if warned.insert(message.clone()) {
                    output::warn(&message);
                }
            }
            Event::Failed { error } => {
                output::failure(ctx, &error.message, &error.next);
                failed.insert(error.message);
            }
            Event::Moved { .. } | Event::Transferred { .. } => {}
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

/// Under the table: what is missing, what is out of date and why, what the file no longer names, and where the cuts are.
fn summary(ctx: &Context, state: &State) {
    for asset in state.assets.iter().filter(|asset| !asset.prefixes.is_empty()) {
        let made: Vec<String> = asset.prefixes.iter().map(|made| format!("{} {} {}", asset.noun, made.number, made.prefix)).collect();
        output::say(ctx, &format!("{}: {}", asset.name, made.join(", ")));
        for made in &asset.prefixes {
            if let Some(kept) = made.kept {
                output::say(ctx, &kept.said(&asset.name, made.number));
            }
        }
    }
    if state.run_files > 0 {
        output::say(ctx, &format!("{} files from `cozy run` here, belonging to no asset: approve one, or `cozy project reclaim` once nothing names them", state.run_files));
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
        let said = format!("project.lock keeps {n} made of {name} (a {kind}), {}, which project.yaml no longer names", output::bytes(orphan.bytes));
        output::say(ctx, &said);
        let to = orphan.renamed_to.as_deref().unwrap_or("<its new name>");
        output::next(ctx, &[&format!("cozy project rename {} {to}", orphan.name)]);
    }
    if !state.orphans.is_empty() {
        output::next(ctx, &["cozy project reclaim"]);
    }
    for cut in &state.cuts {
        let skips = match cut.skips.as_slice() {
            [] => String::new(),
            skips => format!("; it skips {} (nothing made yet)", skips.join(", ")),
        };
        let name = cut.cut.trim_end_matches(".mp4");
        let made = cut.frames.map(|frames| format!(", {}", length(frames, cut.fps))).unwrap_or_default();
        let retimed = retimes(&cut.retimed, cut.fps);
        match cut.cut_at_ms {
            Some(at_ms) => output::say(
                ctx,
                &format!("{name}: {}{made}, cut {} ago{retimed}{skips}", output::link(ctx, &cut.path), output::age(at_ms)),
            ),
            None => output::say(ctx, &format!("{name}: not cut yet ({})", cut.path.display())),
        }
    }
    if state.absent > 0 {
        output::say(ctx, &format!("{} files project.lock names are not on this computer", state.absent));
        output::next(ctx, &["cozy project pull"]);
    } else if !state.missing.is_empty() || !state.stale.is_empty() {
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
