//! `cozy render`: the page's Render, on its route (`POST /v1/projects/{id}/runs`), followed on `GET /v1/events` until what it asked
//! lands (cl-956). It brings the named nodes up to date and what they read first; a node already rendering is joined, so running it
//! again never pays twice, and killing it never cancels the render.
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead as _, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
};

use api::{
    calcifer::{
        NoteLevel, ProjectShowReply, Shown, StreamEvent,
        project::{Asked, AssetVersions, Board, Fit, FitAsk, Job, JobState, Pickup, Runs, Said, Standing, State},
        tool::Tools,
    },
    error::{Error, Kind},
    interface::Snap,
};
use serde_json::{Value, json};

use crate::{
    Context,
    calcifer_client::{Client, Watch, path},
    output,
    run::mint_key,
};

/// A take's frames per second when its file says none, as the page reads it.
const FPS: f64 = 24.0;
/// What a job that made nothing says it came to.
const MADE_NOTHING: [&str; 3] = ["it stands", "nothing to do", "stays pinned"];

const EXAMPLES: &str = "Examples
  cozy render film                    film, and what it reads with no current take first
  cozy render shot_1 shot_2 -n 3      three takes of each that is missing or out of date
  cozy render shot_2 --new            a new take though it is current
  cozy render shot_2 --pickup '<shot_2.1>@243'
                                      Pick up from here: take 1 up to frame 243 kept, the rest rendered again

A node with an approved take stands, unless --outdated. Rendering a node already rendering joins it; stopping this command never
cancels the render (cozy run cancel <run> does), and running it again joins it.";

/// Render nodes and what they read, as the page's Render does, and follow them until their takes land.
#[derive(clap::Args, Debug)]
#[command(after_help = EXAMPLES)]
pub(crate) struct Args {
    /// The nodes to bring up to date.
    #[arg(required = true, value_name = "NAME")]
    names: Vec<String>,
    /// The machines it may go to: `local`, a machine, or several (`a,b`). Default: the project's machines, else any.
    #[arg(long = "on", value_name = "MACHINE[,MACHINE]")]
    on: Option<String>,
    /// Takes of each named node that renders. What they read renders once.
    #[arg(short = 'n', value_name = "N", default_value_t = 1)]
    n: u32,
    /// Render the named nodes though they are current.
    #[arg(long)]
    new: bool,
    /// Render approved takes that are out of date too.
    #[arg(long)]
    outdated: bool,
    /// Pick up from here: keep a take up to a frame and render the rest again. One node.
    #[arg(long, value_name = "<NAME.N>@FRAME")]
    pickup: Option<String>,
}

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    let id = crate::project::identity(ctx).await?.id;
    let calcifer = ctx.calcifer().await?;
    let state: State = calcifer.get(&path(&["v1", "projects", &id])).await?;
    let project = Project { id: state.id.clone(), dir: state.path.clone(), calcifer };
    let n = match args.n {
        0 => {
            output::warn("-n 0: using 1");
            1
        }
        n => n,
    };
    let pickup = match &args.pickup {
        Some(text) => Some(project.pickup(&state, &args.names, text).await?),
        None => None,
    };
    let on = match &args.on {
        Some(on) => api::calcifer::on_set(on)?,
        None => project.here_only(ctx, &state, &args).await?,
    };
    // The page's stream, showing this project, from before the board is read: no change falls between.
    let mut watch = project.watch().await?;
    let head: State = project.calcifer.get(&format!("{}?assets=false", project.at(&[]))).await?;
    let board = head.board.unwrap_or_default();
    let mut follow = Follow::new(ctx, &args.names, &board);
    follow.adopt();
    // With --new a named node rendering already is joined, not rendered twice.
    let (asked, joined): (Vec<String>, Vec<String>) = match args.new {
        true => args.names.iter().cloned().partition(|name| !follow.rendering(&board, name)),
        false => (args.names.clone(), vec![]),
    };
    let mut said = joined.iter().map(|name| Said { level: NoteLevel::Info, message: format!("{name} is rendering: joined"), names: vec![name.clone()] }).collect::<Vec<_>>();
    let mut ask = None;
    if !asked.is_empty() {
        let runs = Runs {
            names: asked,
            n,
            on,
            hub: ctx.hub().to_string(),
            approve: false,
            missing: !args.new,
            outdated: args.outdated,
            pickup,
            takes: vec![],
        };
        let answer: Asked = project.calcifer.post(&project.at(&["runs"]), &runs, Some(&mint_key())).await?;
        follow.board(answer.board.clone());
        said.extend(answer.said.clone());
        ask = answer.ask;
    }
    if follow.mine.is_empty() {
        return Ok(follow.nothing(&said));
    }
    follow.begin(&state, &said);
    loop {
        follow.land(&project).await?;
        if follow.done(ask.as_deref()) {
            break;
        }
        match watch.change().await {
            Some(StreamEvent::BoardUpdated { project: of, board }) if of == project.id => follow.board(*board),
            Some(StreamEvent::JobUpdated { project: of, job, seq }) if of == project.id => follow.job(*job, seq),
            Some(_) => {}
            // Behind or closed, Calcifer restarting too: a new stream first, then the board as it stands, so nothing falls between.
            None => {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                watch = project.watch().await?;
                let head: State = project.calcifer.get(&format!("{}?assets=false", project.at(&[]))).await?;
                if let Some(board) = head.board {
                    follow.board(board);
                }
            }
        }
    }
    follow.end()
}

/// The current project, as its routes name it.
struct Project {
    id: String,
    dir: PathBuf,
    calcifer: Client,
}

impl Project {
    fn at(&self, rest: &[&str]) -> String {
        path(&[["v1", "projects", self.id.as_str()].as_slice(), rest].concat())
    }

    /// `GET /v1/events`, showing this project as the page does (`PUT /v1/events/{stream}`).
    async fn watch(&self) -> Result<Watch<StreamEvent>, Error> {
        let mut watch = self.calcifer.changes().await?;
        let Some(StreamEvent::Hello { stream, .. }) = watch.change().await else {
            return Err(Error::new(Kind::Unavailable, "events_unanswered", "Calcifer's event stream ended before it began").next("cozy doctor"));
        };
        let shown: ProjectShowReply = self.calcifer.put(&path(&["v1", "events", &stream]), &Shown { projects: vec![self.id.clone()] }).await?;
        match shown.refused.into_values().next() {
            Some(refused) => Err(refused.error),
            None => Ok(watch),
        }
    }

    /// `--pickup '<shot_2.1>@243'`, as the page's Pick up from here sends it: the frame on the frames the node's tool lands a pickup on,
    /// and its beats from there; another node's take goes on from its beats' start.
    async fn pickup(&self, state: &State, names: &[String], text: &str) -> Result<Pickup, Error> {
        let [name] = names else {
            return Err(usage("pickup_one_node", format!("--pickup is for one node, not {}", names.len())));
        };
        let malformed = || usage("pickup_malformed", format!("--pickup {text}: it's '<node.N>@frame', as '<shot_1.1>@243'"));
        let (take, frame) = text.trim().rsplit_once('@').ok_or_else(malformed)?;
        let frame: u32 = frame.trim().parse().map_err(|_| malformed())?;
        let (of, number) = take.trim().trim_start_matches('<').trim_end_matches('>').rsplit_once('.').ok_or_else(malformed)?;
        let number: u32 = number.parse().map_err(|_| malformed())?;
        let source = row(state, of).ok_or_else(|| Error::new(Kind::NotFound, "pickup_no_take", format!("--pickup: the project has no {of}")).next("cozy get"))?;
        let main = source.prefixes.iter().find(|prefix| prefix.number == number).and_then(|prefix| prefix.main.as_ref()).filter(|main| main.field == "video");
        let Some(main) = main else {
            return Err(Error::new(Kind::NotFound, "pickup_no_take", format!("{} has no take {number} with a video", source.name)).next(format!("cozy get {} --takes", source.name)));
        };
        let node = row(state, name).ok_or_else(|| Error::new(Kind::NotFound, "asset_unknown", format!("the project has no {name}")).next("cozy get"))?;
        let tools: Tools = self.calcifer.get_query(&path(&["v1", "tools"]), &[("project", self.id.as_str())]).await?;
        // Local code its tool list doesn't name picks up on every frame, which its binder holds to.
        let snap = match node.tool.as_deref().and_then(|tool| tools.tools.into_iter().find(|listed| listed.id == tool)) {
            Some(listed) => listed.pickup.ok_or_else(|| usage("pickup_untaken", format!("{}'s tool takes no pickup", node.name)))?,
            None => Snap::default(),
        };
        let fps = main.fps.filter(|fps| *fps > 0.0).unwrap_or(FPS);
        let last = ((main.duration_ms.unwrap_or_default() as f64 / 1000.0) * fps).round().max(1.0) as u32;
        let Some(at) = pickup_at(frame, last, snap) else {
            return Err(usage("pickup_too_soon", format!("frame {frame} is before the first frame {}'s tool picks up at, {}", node.name, snap.first)));
        };
        if at != frame {
            output::warn(&format!("frame {frame} isn't one a pickup lands on: it picks up at {at}"));
        }
        let own = key(&source.name) == key(&node.name);
        Ok(Pickup { take: format!("<{}.{number}>", source.name), out_frame: at, beats_from: Some(if own { f64::from(at) / fps } else { 0.0 }) })
    }

    /// The machines a render naming none goes to, as Calcifer places it (cl-894): with this computer's alone, whether it would do it
    /// poorly is asked first, as the page asks, and off a terminal that is a refusal naming what to run instead.
    async fn here_only(&self, ctx: &Context, state: &State, args: &Args) -> Result<Vec<api::calcifer::On>, Error> {
        let set = match state.machines.is_empty() {
            false => state.machines.clone(),
            true => self.calcifer.machines().await?.render_set().iter().map(ToString::to_string).collect(),
        };
        if set != ["local"] {
            return Ok(vec![]);
        }
        let ask = FitAsk { names: args.names.clone(), outdated: args.outdated };
        let fit: Fit = self.calcifer.post(&self.at(&["fit"]), &ask, None).await?;
        let poorly: Vec<_> = fit.tools.iter().filter(|tool| !tool.here).collect();
        if poorly.is_empty() {
            return Ok(vec![]);
        }
        let names: Vec<&str> = poorly.iter().flat_map(|tool| tool.names.iter().map(String::as_str)).collect();
        let lines: Vec<&str> = poorly.iter().map(|tool| tool.line.as_str()).collect();
        let here = format!("cozy render {} --on local", args.names.join(" "));
        let rent = fit.rent.as_ref().map_or_else(|| "cozy machine rent".to_owned(), |rent| format!("cozy machine rent {}", rent.sku));
        if ctx.yes || (output::can_ask(ctx) && here_anyway(&lines, fit.rent.as_ref().map(|rent| rent.line.as_str()))) {
            let _: Fit = self.calcifer.post(&self.at(&["fit", "here"]), &ask, None).await?;
            return Ok(vec![api::calcifer::On::Machine("local".into())]);
        }
        let said = format!("{} may not fit this computer's GPU, and nothing is rented: {}", and(&names), lines.join("; "));
        Err(Error::new(Kind::Capacity, "local_fit", said).next(format!("{here}, or {rent}")))
    }

    /// One take of `name`, as the project holds it now.
    async fn versions(&self, name: &str) -> Result<AssetVersions, Error> {
        self.calcifer.get(&self.at(&["assets", name])).await
    }
}

/// The page's question, on a terminal: render here anyway?
fn here_anyway(lines: &[&str], rent: Option<&str>) -> bool {
    for line in lines {
        anstream::eprintln!("{line}");
    }
    if let Some(rent) = rent {
        anstream::eprintln!("{rent}");
    }
    anstream::eprint!("Render here anyway? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes")
}

/// What this render follows, as the board says it: each job of the named nodes and of what they wait for, said once as it lands.
struct Follow<'a> {
    ctx: &'a Context,
    names: Vec<String>,
    jobs: Vec<Job>,
    seq: u64,
    /// Ended before this render was asked: another's.
    before: HashSet<String>,
    /// The jobs it follows, in the order it found them.
    mine: Vec<String>,
    /// Each one's line, said once.
    said: HashSet<String>,
    /// What a person on a terminal was last told of each.
    told: HashMap<String, String>,
    lines: usize,
    /// When the walk its board is of began: another's says its lines from the first.
    began: i64,
    ended: Option<api::calcifer::project::Ended>,
    takes: Vec<Value>,
    failed: Vec<Job>,
    /// The widest `name take N`, so lines line up.
    width: usize,
}

impl<'a> Follow<'a> {
    fn new(ctx: &'a Context, names: &[String], board: &Board) -> Self {
        let before = board.jobs.iter().filter(|job| ended(job.state)).map(|job| job.id.clone()).collect();
        Self {
            ctx,
            names: names.iter().map(|name| key(name)).collect(),
            jobs: board.jobs.clone(),
            seq: board.seq,
            before,
            mine: vec![],
            said: HashSet::new(),
            told: HashMap::new(),
            lines: board.said.len(),
            began: board.began_at_ms,
            ended: board.ended.clone(),
            takes: vec![],
            failed: vec![],
            width: 0,
        }
    }

    /// Whether `name` has a job not ended.
    fn rendering(&self, board: &Board, name: &str) -> bool {
        board.jobs.iter().any(|job| key(&job.asset) == key(name) && !ended(job.state))
    }

    fn board(&mut self, board: Board) {
        if board.seq < self.seq {
            return;
        }
        self.seq = board.seq;
        if board.began_at_ms != self.began {
            (self.began, self.lines) = (board.began_at_ms, 0);
        }
        self.jobs = board.jobs;
        self.ended = board.ended;
        for said in board.said.iter().skip(self.lines) {
            match said.level {
                NoteLevel::Warning => output::warn(&said.message),
                NoteLevel::Info if self.progress() => output::say(self.ctx, &said.message),
                NoteLevel::Info => {}
            }
        }
        self.lines = self.lines.max(board.said.len());
        self.adopt();
    }

    fn job(&mut self, job: Job, seq: u64) {
        if seq < self.seq {
            return;
        }
        self.seq = seq;
        match self.jobs.iter_mut().find(|held| held.id == job.id) {
            Some(held) => *held = job,
            None => self.jobs.push(job),
        }
        self.adopt();
    }

    /// The jobs of the named nodes, and of what those wait for, all the way up: each is followed from when it's first seen.
    fn adopt(&mut self) {
        loop {
            let mine: HashSet<&str> = self.mine.iter().map(String::as_str).collect();
            let wanted: HashSet<String> = (self.jobs.iter().filter(|job| mine.contains(job.id.as_str())))
                .flat_map(|job| job.needs.iter().map(|name| key(name)))
                .chain(self.names.iter().cloned())
                .collect();
            let found: Vec<String> = (self.jobs.iter())
                .filter(|job| !job.id.is_empty() && !mine.contains(job.id.as_str()) && !self.before.contains(&job.id) && wanted.contains(&key(&job.asset)))
                .map(|job| job.id.clone())
                .collect();
            if found.is_empty() {
                break;
            }
            self.mine.extend(found);
        }
        for job in self.jobs.iter().filter(|job| self.mine.contains(&job.id)) {
            self.width = self.width.max(job.asset.len() + format!(" take {}", job.number.unwrap_or(1)).len());
        }
        self.tell();
    }

    /// What it renders first, and why: what the named nodes read that renders, and what they joined.
    fn begin(&self, state: &State, said: &[Said]) {
        if self.ctx.json {
            output::ndjson(&json!({"type": "asked", "jobs": self.mine_jobs().map(|job| json!({"name": job.asset, "state": job.state})).collect::<Vec<_>>(), "said": said}));
            return;
        }
        for said in said.iter().filter(|said| said.level == NoteLevel::Warning) {
            output::warn(&said.message);
        }
        // Named nodes that render nothing say why, when something else renders.
        for said in said.iter().filter(|said| said.level == NoteLevel::Info && said.names.len() == 1) {
            let rendering = self.mine_jobs().any(|job| key(&job.asset) == key(&said.names[0]));
            if !rendering || said.message.ends_with("joined") {
                anstream::println!("{}", said.message);
            }
        }
        let mut deps: Vec<&str> = vec![];
        for job in self.mine_jobs().filter(|job| !self.names.contains(&key(&job.asset))) {
            if !deps.contains(&job.asset.as_str()) {
                deps.push(&job.asset);
            }
        }
        // Each under the node that reads it, from the named nodes up, in the order its text mentions them.
        let mut readers: Vec<&str> = self.mine_jobs().filter(|job| self.names.contains(&key(&job.asset))).map(|job| job.asset.as_str()).collect();
        let (mut at, mut placed) = (0, HashSet::new());
        while let Some(&reader) = readers.get(at) {
            at += 1;
            let reads: Vec<&str> = row(state, reader).into_iter().flat_map(|asset| &asset.reads).filter(|read| read.version.is_none())
                .filter_map(|read| deps.iter().copied().find(|dep| key(dep) == key(&read.name)))
                .filter(|dep| placed.insert(key(dep)))
                .collect();
            if reads.is_empty() {
                continue;
            }
            readers.extend(reads.iter().copied());
            let whys: Vec<&str> = reads.iter().map(|name| why(state, name)).collect();
            let said = match whys.iter().all(|why| *why == whys[0]) {
                true => format!("{reader} reads {}, which {}: rendering {} first", and(&reads), plural(whys[0], reads.len()), if reads.len() == 1 { "it" } else { "them" }),
                false => {
                    let each: Vec<String> = reads.iter().zip(&whys).map(|(name, why)| format!("{name}, which {why}")).collect();
                    format!("{reader} reads {}: rendering them first", each.join(", and "))
                }
            };
            anstream::println!("{said}");
        }
        let rest: Vec<&str> = deps.into_iter().filter(|dep| !placed.contains(&key(dep))).collect();
        if !rest.is_empty() {
            anstream::println!("rendering {} first", and(&rest));
        }
    }

    fn mine_jobs(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter().filter(|job| self.mine.contains(&job.id))
    }

    /// Nothing to render: the named nodes each said why.
    fn nothing(&self, said: &[Said]) -> ExitCode {
        if !self.ctx.json {
            said.iter().filter(|said| said.level == NoteLevel::Warning).for_each(|said| output::warn(&said.message));
        }
        let why: Vec<&str> = said.iter().filter(|said| said.level == NoteLevel::Info).map(|said| said.message.as_str()).collect();
        let line = match why.is_empty() {
            true => "nothing to render".to_owned(),
            false => format!("nothing to render: {}", why.join("; ")),
        };
        if self.ctx.json {
            output::ndjson(&json!({"type": "rendered", "takes": [], "nothing": line}));
        }
        output::nothing(self.ctx, &line);
        ExitCode::SUCCESS
    }

    /// Whether everything it follows ended, and each take it made is here: else, once the walk that made it ends. One gone from the board
    /// went with a walk that ended.
    fn done(&self, ask: Option<&str>) -> bool {
        let walked = self.ended.as_ref().is_some_and(|ended| ask.is_none_or(|ask| ended.asks.iter().any(|held| held == ask)));
        let gone = |id: &String| !self.jobs.iter().any(|job| &job.id == id);
        self.mine_jobs().all(|job| ended(job.state)) && (walked || self.mine.iter().all(|id| self.said.contains(id) || gone(id)))
    }

    /// Each job it follows that ended, said once: a take, once its file is here.
    async fn land(&mut self, project: &Project) -> Result<(), Error> {
        let walked = self.ended.is_some();
        let ended: Vec<Job> = self.mine_jobs().filter(|job| ended(job.state) && !self.said.contains(&job.id)).cloned().collect();
        for job in ended {
            let file = job.path.as_deref().map(|at| project.dir.join(at));
            if job.state == JobState::Done && file.as_ref().is_some_and(|file| !file.exists()) && !walked {
                continue;
            }
            self.said.insert(job.id.clone());
            match job.state {
                JobState::Done => self.landed(project, &job, file.as_deref()).await?,
                _ => self.stopped(job),
            }
        }
        Ok(())
    }

    async fn landed(&mut self, project: &Project, job: &Job, file: Option<&Path>) -> Result<(), Error> {
        let label = label(job);
        let Some(number) = job.number else {
            self.line(&label, job.why.as_deref().unwrap_or("done"), json!({"name": job.asset, "state": job.state}));
            return Ok(());
        };
        if job.why.as_deref().is_some_and(|why| MADE_NOTHING.contains(&why)) {
            let why = job.why.clone().unwrap_or_default();
            self.line(&label, &why, json!({"name": job.asset, "number": number, "state": "stands", "why": why}));
            return Ok(());
        }
        let held = project.versions(&job.asset).await.ok();
        let main = held.as_ref().and_then(|held| held.asset.prefixes.iter().find(|prefix| prefix.number == number)).and_then(|prefix| prefix.main.clone());
        let version = held.as_ref().and_then(|held| held.versions.iter().find(|version| version.number == number));
        let seconds = job.seconds.or_else(|| main.as_ref().and_then(|main| main.duration_ms).filter(|ms| *ms > 0).map(|ms| ms as f64 / 1000.0));
        let seams = match (job.run, main.as_ref().is_some_and(|main| main.field == "video")) {
            (Some(run), true) => project.calcifer.run(&crate::calcifer_client::RunAddress { project: Some(project.id.clone()), run: run.to_string() }).await.ok().and_then(|run| seams(run.result.as_ref())),
            _ => None,
        };
        let took = job.took_ms.or_else(|| job.started_at_ms.map(|at| (now_ms() - at).max(0) as u64));
        let mut what = vec![format!("done in {:<5}", took.map_or_else(|| "-".to_owned(), clock))];
        let mut about = vec![];
        match (seconds, main.as_ref().and_then(|main| main.width.zip(main.height))) {
            (Some(seconds), _) => about.push(format!("{seconds:.1} s")),
            (None, Some((width, height))) => about.push(format!("{width}x{height}")),
            _ => {}
        }
        match seams.as_deref() {
            Some([seam]) => about.push(format!("seam at frame {seam}")),
            Some(more) if !more.is_empty() => about.push(format!("seams at frames {}", more.iter().map(u32::to_string).collect::<Vec<_>>().join(", "))),
            _ => {}
        }
        if !about.is_empty() {
            what.push(format!("{:<7}", about.join(", ")));
        }
        let file = file.map(|file| file.display().to_string());
        if let Some(file) = &file {
            what.push(file.clone());
        }
        let warnings = version.map(|version| version.warnings.clone()).unwrap_or_default();
        let take = json!({
            "name": job.asset, "number": number, "state": "done", "file": file, "run": job.run, "took_ms": took, "seconds": seconds,
            "seams": seams, "warnings": warnings,
        });
        self.line(&label, &what.join("  "), take.clone());
        if !self.ctx.json {
            warnings.iter().for_each(|warning| output::warn(warning));
        }
        self.takes.push(take);
        Ok(())
    }

    fn stopped(&mut self, job: Job) {
        let label = label(&job);
        let state = json!(job.state).as_str().unwrap_or_default().replace('_', " ");
        let why = job.error.as_ref().map(|error| error.message.clone()).or_else(|| job.why.clone()).map(|why| own(&job.asset, &why));
        let what = match &why {
            Some(why) => format!("{state}: {why}"),
            None => state,
        };
        self.line(&label, &what, json!({"name": job.asset, "number": job.number, "state": job.state, "run": job.run, "error": job.error, "why": job.why}));
        self.failed.push(job);
    }

    fn line(&self, label: &str, what: &str, take: Value) {
        match self.ctx.json {
            true => output::ndjson(&json!({"type": "take", "take": take})),
            false => anstream::println!("{label:<width$}  {what}", width = self.width),
        }
    }

    /// Per-step progress, for a person watching a terminal.
    fn progress(&self) -> bool {
        !self.ctx.json && self.ctx.term.stdout && self.ctx.term.stderr
    }

    fn tell(&mut self) {
        if !self.progress() {
            return;
        }
        let jobs: Vec<Job> = self.mine_jobs().filter(|job| !ended(job.state)).cloned().collect();
        for job in jobs {
            let now = match job.state {
                JobState::Waiting if !job.needs.is_empty() => format!("waits for {}", and(&job.needs.iter().map(String::as_str).collect::<Vec<_>>())),
                JobState::Waiting => job.why.clone().unwrap_or_else(|| "queued".to_owned()),
                _ => {
                    let at = job.machine.as_deref().map_or_else(String::new, |machine| format!(" on {machine}"));
                    let stage = [job.stage.as_deref(), job.detail.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" ");
                    if stage.is_empty() { format!("running{at}") } else { format!("running{at}: {stage}") }
                }
            };
            if self.told.get(&job.id) != Some(&now) {
                output::say(self.ctx, &format!("{}: {now}", label(&job)));
                self.told.insert(job.id.clone(), now);
            }
        }
    }

    /// Its answer: the next step once every take landed, else the first thing that failed.
    fn end(self) -> Result<ExitCode, Error> {
        if let Some(job) = self.failed.first() {
            let names: Vec<&str> = self.failed.iter().map(|job| job.asset.as_str()).collect();
            let mut error = job.error.clone().unwrap_or_else(|| Error::new(Kind::Unavailable, "render_stopped", format!("{} {}", job.asset, job.why.clone().unwrap_or_else(|| "stopped".into()))));
            error.message = format!("{} did not render: {}", and(&names), own(&job.asset, &error.message));
            if error.next.is_empty() && let Some(run) = job.run {
                error = error.next(format!("cozy run show {run}"));
            }
            return Err(error);
        }
        let last = self.takes.iter().rev().find(|take| take["name"].as_str().is_some_and(|name| self.names.contains(&key(name)))).or(self.takes.last());
        let next = last.and_then(|take| Some(format!("look at {}; approve with approved: {} and apply", take["file"].as_str()?, take["number"])));
        match self.ctx.json {
            true => output::ndjson(&json!({"type": "rendered", "takes": self.takes, "next": next})),
            false => {
                if let Some(next) = &next {
                    output::next(self.ctx, &[next.as_str()]);
                }
            }
        }
        Ok(ExitCode::SUCCESS)
    }
}

/// What's said of `name`, without its name and a `failed:` the line says already.
fn own(name: &str, why: &str) -> String {
    let why = why.strip_prefix(name).map_or(why, |rest| rest.trim_start_matches(':').trim_start());
    why.strip_prefix("failed: ").unwrap_or(why).to_owned()
}

/// `shot_2 take 2`; a job with no number yet, its node.
fn label(job: &Job) -> String {
    match job.number {
        Some(number) => format!("{} take {number}", job.asset),
        None => job.asset.clone(),
    }
}

fn ended(state: JobState) -> bool {
    matches!(state, JobState::Done | JobState::Failed | JobState::Canceled | JobState::NotSent)
}

/// Why a node read renders first, as its row says.
fn why<'s>(state: &'s State, name: &str) -> &'s str {
    match row(state, name).map(|asset| (asset.standing, asset.approved.is_some())) {
        Some((Standing::OutOfDate, _)) => "is out of date",
        Some((Standing::Current, true)) => "is approved and out of date",
        Some((Standing::Making, _)) => "is rendering",
        _ => "has no takes",
    }
}

/// `has no takes` said of several: `have no takes`.
fn plural(why: &str, n: usize) -> String {
    match n {
        1 => why.to_owned(),
        _ => why.replacen("has ", "have ", 1).replacen("is ", "are ", 1),
    }
}

fn row<'s>(state: &'s State, name: &str) -> Option<&'s api::calcifer::project::Asset> {
    state.assets.iter().find(|asset| key(&asset.name) == key(name))
}

/// How a name matches a mention, as Calcifer reads names: any case, `_`, `-` and space alike.
fn key(name: &str) -> String {
    name.replace(['_', '-'], " ").split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// `a`, `a and b`, `a, b and c`.
fn and(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `3m40s`, `2s`, `1h02m`.
fn clock(ms: u64) -> String {
    let s = (ms + 500) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

/// Where each segment after the first starts, as the run that made a take said it (`result.seams`, se-130).
fn seams(result: Option<&Value>) -> Option<Vec<u32>> {
    let seams = result?.get("seams")?.as_array()?;
    seams.iter().map(|seam| seam.as_u64().and_then(|seam| u32::try_from(seam).ok())).collect()
}

/// The page's `pickupAt`: `frame` (no later than `last`) on the frames `snap` picks up at, at or before it; none before its first.
fn pickup_at(frame: u32, last: u32, snap: Snap) -> Option<u32> {
    let (at, every, offset, first) = (i64::from(frame.min(last)), i64::from(snap.every.max(1)), i64::from(snap.offset), i64::from(snap.first));
    (at >= first).then(|| first.max(offset + (at - offset).div_euclid(every) * every) as u32)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64)
}

fn usage(code: &str, message: impl Into<String>) -> Error {
    Error::new(Kind::Usage, code, message)
}
