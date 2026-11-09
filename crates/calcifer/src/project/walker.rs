//! The walker's graph (cl-770): one job per version a render owes, made when it is asked and moved by what happens. Most jobs are one-offs
//! running side by side, and assets come and go while they do. It reads a project only through `Frozen`, so neither its format nor where
//! its versions are kept reach this. Nothing here walks the project per event: an event touches its job's readers, and a job's turn looks
//! only at what it reads.
use std::collections::{BTreeSet, HashMap, HashSet};

use sha2::{Digest as _, Sha256};

use super::graph::name_key;

/// One thing a project's text says, all the walker knows of it. A render's nodes never change: its text is frozen when it is asked.
#[derive(Clone, Debug, Default)]
pub(crate) struct Node {
    pub name: String,
    /// What it says of itself, as a digest: two renders' jobs with the same one make the same thing.
    pub spec: String,
    /// What it reads, each before it in `Frozen::order`.
    pub reads: Vec<usize>,
    /// Something makes it.
    pub makes: bool,
    /// Calcifer makes it, free, with no machine: it reads what stands, and is held up only by what is on its way.
    pub free: bool,
    /// Its state is a file the text names, and nothing moves it.
    pub pinned: bool,
    /// It makes a shot.
    pub shot: bool,
    /// What it makes is a video.
    pub video: bool,
    /// How much it makes, as what it takes goes by it: a shot's seconds; one of anything else.
    pub length: f64,
}

/// What the walker reads of one frozen render, and what stands now.
pub(crate) trait Frozen {
    fn nodes(&self) -> &[Node];
    /// Every node, each after what it reads.
    fn order(&self) -> &[usize];
    /// Whether node `i` has a state now.
    fn holds(&mut self, i: usize) -> bool;
    /// A pin this render looks at when its turn comes, to re-run it if it is out of date (`--outdated`).
    fn judged(&mut self, i: usize) -> bool;
}

/// What a job is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A new version asked of it by name.
    Take,
    /// It has no state, and what the render covers reads it.
    Owed,
    /// Calcifer makes it. `named`: though it has a state.
    Free { named: bool },
    /// A pin looked at when its turn comes.
    Judged,
}

/// How a job stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// For what it needs, or its turn.
    Waiting,
    /// It was given its turn: it is being sent, is on its machine, or Calcifer is making it.
    Going,
    Ended(End),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum End {
    Done,
    Failed,
    /// It stopped before it went: canceled, or its asset was deleted.
    Dropped,
    /// Nothing was left to do: it has its state.
    Stands,
}

/// One version of one asset being made.
#[derive(Clone, Debug)]
pub(crate) struct Job {
    /// The key its first run is sent under: one job, one run, ever.
    pub key: String,
    pub ask: String,
    pub node: usize,
    pub asset: String,
    pub spec: String,
    pub kind: Kind,
    pub state: State,
    pub shot: bool,
    pub video: bool,
    /// Pinned when it lands: what reads it waits for that.
    pub approve: bool,
    /// The order things go: the sooner asked and the earlier in the file, the smaller.
    pub rank: u64,
    /// What it waits for, as jobs of this graph.
    pub needs: Vec<usize>,
    unmet: usize,
    readers: Vec<usize>,
}

/// One new version a render asked by name.
#[derive(Clone, Debug)]
pub(crate) struct Named {
    pub key: String,
    pub asset: String,
    pub approve: bool,
}

/// What a render owes as it stands: what it asked, less what it has sent.
#[derive(Clone, Debug, Default)]
pub(crate) struct Asked {
    /// Every asset with no state.
    pub all: bool,
    /// What it has tried, by name key: each is tried once.
    pub tried: BTreeSet<String>,
    pub takes: Vec<Named>,
}

/// What a job's turn came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Turn {
    Go,
    /// It waits for something that came up since it was made.
    Wait,
    /// It has its state: nothing to do.
    Stands,
    /// Node `read`, which it reads, has no state and nothing is making it.
    Unread { read: usize },
}

/// A project's jobs, all its renders' at once.
#[derive(Default)]
pub(crate) struct Graph {
    pub jobs: Vec<Job>,
    by_key: HashMap<String, usize>,
    /// Jobs not ended, by asset's name key.
    open: HashMap<String, Vec<usize>>,
    /// Waiting with nothing unmet, by rank.
    ready: BTreeSet<(u64, usize)>,
    /// Jobs not ended, by render.
    left: HashMap<String, usize>,
    /// What each render has tried, by name key.
    tried: HashMap<String, BTreeSet<String>>,
    /// A job ended since the walk last said so.
    pub ended: bool,
}

impl Graph {
    pub(crate) fn job(&self, key: &str) -> Option<&Job> {
        self.by_key.get(key).map(|&id| &self.jobs[id])
    }

    pub(crate) fn id(&self, key: &str) -> Option<usize> {
        self.by_key.get(key).copied()
    }

    /// Whether render `ask` has anything left.
    pub(crate) fn owes(&self, ask: &str) -> bool {
        self.left.get(ask).is_some_and(|left| *left > 0)
    }

    /// How many jobs are going.
    pub(crate) fn going(&self) -> usize {
        self.open.values().flatten().filter(|&&id| self.jobs[id].state == State::Going).count()
    }

    /// The name keys of the assets with jobs not ended.
    pub(crate) fn open_names(&self) -> impl Iterator<Item = &String> {
        self.open.keys()
    }

    /// Whether a job is ready for its turn.
    pub(crate) fn has_ready(&self) -> bool {
        !self.ready.is_empty()
    }

    /// Jobs not ended, in the order they go: what is going, then what waits.
    pub(crate) fn open_jobs(&self) -> Vec<usize> {
        let mut open: Vec<usize> = self.open.values().flatten().copied().collect();
        open.sort_by_key(|&id| (self.jobs[id].state != State::Going, self.jobs[id].rank, id));
        open
    }

    /// Those of its needs that have not ended, by name: what a person reads of what it waits for.
    pub(crate) fn waits_for(&self, id: usize) -> Vec<&str> {
        let mut names: Vec<&str> = vec![];
        for &need in &self.jobs[id].needs {
            let job = &self.jobs[need];
            if !matches!(job.state, State::Ended(_)) && !names.contains(&job.asset.as_str()) {
                names.push(&job.asset);
            }
        }
        names
    }

    /// A job that was on its way before this graph: a run a walker before this one sent. It goes on as it was.
    pub(crate) fn adopt(&mut self, ask: &str, key: &str, asset: &str, node: &Node, approve: bool) -> usize {
        let id = self.make(Job {
            key: key.to_owned(),
            ask: ask.to_owned(),
            node: usize::MAX,
            asset: asset.to_owned(),
            spec: node.spec.clone(),
            kind: Kind::Take,
            state: State::Going,
            shot: node.shot,
            video: node.video,
            approve,
            rank: 0,
            needs: vec![],
            unmet: 0,
            readers: vec![],
        });
        self.ready.remove(&(0, id));
        id
    }

    /// Render `ask`: a job for each take it names and each thing it owes, with what each needs.
    pub(crate) fn add(&mut self, ask: &str, frozen: &mut dyn Frozen, asked: &Asked) -> Vec<usize> {
        let nodes: Vec<Node> = frozen.nodes().to_vec();
        let order: Vec<usize> = frozen.order().to_vec();
        let keys: Vec<String> = nodes.iter().map(|node| name_key(&node.name)).collect();
        let by_name: HashMap<&str, usize> = keys.iter().enumerate().map(|(i, key)| (key.as_str(), i)).collect();
        let n = nodes.len();
        let mut taken = vec![vec![]; n];
        for take in &asked.takes {
            if let Some(&i) = by_name.get(name_key(&take.asset).as_str()) {
                taken[i].push(take);
            }
        }
        let holds: Vec<bool> = (0..n).map(|i| frozen.holds(i)).collect();
        let judged: Vec<bool> = (0..n).map(|i| !asked.tried.contains(&keys[i]) && frozen.judged(i)).collect();
        let tried = |i: usize| asked.tried.contains(&keys[i]);
        self.tried.entry(ask.to_owned()).or_default().extend(asked.tried.iter().cloned());
        let mut covered = vec![asked.all; n];
        for &i in order.iter().rev() {
            let node = &nodes[i];
            let named = !taken[i].is_empty();
            // Naming a cut cuts what stands, free: it pays for nothing it reads.
            covered[i] |= named && !node.free;
            if covered[i] && (!holds[i] || named || (node.free && !node.pinned)) {
                for &read in &node.reads {
                    covered[read] = true;
                }
            }
        }
        // Each node's jobs, with what they need: what makes what they read. A node with none passes on what it reads.
        let base = self.jobs.len() as u64 + 1;
        let mut through: Vec<Vec<usize>> = vec![vec![]; n];
        let mut made = vec![];
        for (place, &i) in order.iter().enumerate() {
            let node = &nodes[i];
            let named = !taken[i].is_empty();
            // A pin needs nothing it reads, unless a take of it does.
            let up: Vec<usize> = match node.pinned && !named && !judged[i] {
                true => vec![],
                false => merged(node.reads.iter().map(|&read| &through[read])),
            };
            let job = |key: String, kind: Kind, approve: bool, up: &[usize]| Job {
                key,
                ask: ask.to_owned(),
                node: i,
                asset: node.name.clone(),
                spec: node.spec.clone(),
                kind,
                state: State::Waiting,
                shot: node.shot,
                video: node.video,
                approve,
                rank: base + place as u64,
                needs: up.to_vec(),
                unmet: 0,
                readers: vec![],
            };
            let mut own = vec![];
            for take in &taken[i] {
                // Already going, as what a walker before this one sent.
                if self.by_key.contains_key(&take.key) {
                    continue;
                }
                let kind = if node.free { Kind::Free { named: true } } else { Kind::Take };
                own.push(self.make(job(take.key.clone(), kind, take.approve, &up)));
            }
            // What another render is making of the same thing answers what has no state here, and what reads it waits for that.
            let others: Vec<usize> = (self.open.get(&keys[i]).into_iter().flatten())
                .copied()
                .filter(|&id| self.jobs[id].ask != ask && self.jobs[id].spec == node.spec)
                .collect();
            let owed = covered[i] && !holds[i] && node.makes && !node.free && !tried(i) && others.is_empty();
            let free = node.free && node.makes && !node.pinned && !tried(i) && (!holds[i] || !up.is_empty());
            if !named && (owed || free || judged[i]) {
                let kind = match (free, judged[i]) {
                    (true, _) => Kind::Free { named: false },
                    (_, true) => Kind::Judged,
                    _ => Kind::Owed,
                };
                let key = owed_key(ask, &keys[i]);
                if !self.by_key.contains_key(&key) {
                    own.push(self.make(job(key, kind, false, &up)));
                }
            }
            made.extend(own.iter().copied());
            // Going already, of this render, as what a walker before this one sent.
            let going = self.open.get(&keys[i]).into_iter().flatten().copied().filter(|&id| self.jobs[id].ask == ask && self.jobs[id].state == State::Going);
            own.extend(going);
            let approving = own.iter().any(|&id| self.jobs[id].approve);
            let holds_up = approving || judged[i] || (!node.pinned && !own.is_empty());
            through[i] = match (holds_up, node.pinned, others.is_empty()) {
                (true, _, true) => own,
                (true, _, false) => merged([&own, &others].into_iter()),
                (false, false, false) => merged([&up, &others].into_iter()),
                (false, ..) => up,
            };
        }
        for &id in &made {
            self.linked(id);
        }
        made
    }

    /// A job's turn, looking only at what it reads.
    pub(crate) fn turn(&mut self, id: usize, frozen: &mut dyn Frozen) -> Turn {
        let (ask, node, kind) = (self.jobs[id].ask.clone(), self.jobs[id].node, self.jobs[id].kind);
        let nodes = frozen.nodes();
        let free = nodes[node].free;
        let reads: Vec<usize> = match nodes[node].pinned && kind == Kind::Owed {
            true => vec![],
            false => nodes[node].reads.clone(),
        };
        if free {
            return Turn::Go;
        }
        // What it reads with no state is made first, if it can be: one a render covers, or that came up since.
        let mut spawned = vec![];
        let mut missing = None;
        for read in reads {
            if frozen.holds(read) {
                continue;
            }
            let before = spawned.len();
            self.owed_upstream(read, &ask, frozen, &mut spawned);
            if spawned.len() == before {
                missing.get_or_insert(read);
            }
        }
        if !spawned.is_empty() {
            self.wait_on(id, spawned);
            return Turn::Wait;
        }
        if let Some(read) = missing {
            return Turn::Unread { read };
        }
        if kind == Kind::Owed && frozen.holds(node) {
            return Turn::Stands;
        }
        Turn::Go
    }

    /// Back to waiting for its turn: a machine isn't free for it yet, or sending it found out what it is.
    pub(crate) fn put_back(&mut self, id: usize) {
        self.jobs[id].state = State::Waiting;
        self.ready.insert((self.jobs[id].rank, id));
    }

    /// What the jobs not ended of the renders `asks` names are, now their text is read knowing more.
    pub(crate) fn read_again(&mut self, asks: impl Fn(&str) -> bool, nodes: &[Node]) {
        let open: Vec<usize> = self.open.values().flatten().copied().filter(|&id| asks(&self.jobs[id].ask)).collect();
        for id in open {
            let job = &mut self.jobs[id];
            if let Some(node) = nodes.get(job.node) {
                job.spec.clone_from(&node.spec);
                (job.shot, job.video) = (node.shot, node.video);
            }
        }
    }

    /// The jobs ready for their turn, smallest rank first, at most `room`. Each stays the caller's until `going`, `end` or it waits again.
    pub(crate) fn take_ready(&mut self, room: usize) -> Vec<usize> {
        let taken: Vec<(u64, usize)> = self.ready.iter().take(room).copied().collect();
        for entry in &taken {
            self.ready.remove(entry);
        }
        taken.into_iter().map(|(_, id)| id).collect()
    }

    /// Its turn came to something that goes.
    pub(crate) fn go(&mut self, id: usize) {
        self.jobs[id].state = State::Going;
    }

    /// It ended: what it holds up may be ready now.
    pub(crate) fn end(&mut self, id: usize, end: End) {
        if matches!(self.jobs[id].state, State::Ended(_)) {
            return;
        }
        self.ready.remove(&(self.jobs[id].rank, id));
        self.jobs[id].state = State::Ended(end);
        self.ended = true;
        let key = name_key(&self.jobs[id].asset);
        if let Some(open) = self.open.get_mut(&key) {
            open.retain(|&other| other != id);
            if open.is_empty() {
                self.open.remove(&key);
            }
        }
        if let Some(left) = self.left.get_mut(&self.jobs[id].ask) {
            *left = left.saturating_sub(1);
        }
        let ask = self.jobs[id].ask.clone();
        self.tried.entry(ask).or_default().insert(key);
        for reader in std::mem::take(&mut self.jobs[id].readers) {
            let job = &mut self.jobs[reader];
            job.unmet = job.unmet.saturating_sub(1);
            if job.unmet == 0 && job.state == State::Waiting {
                self.ready.insert((job.rank, reader));
            }
        }
    }

    /// Those waiting that `wanted` names leave, and what reads them finds it at its turn. Answers them, and those going, whose runs are the caller's to cancel.
    pub(crate) fn cancel(&mut self, wanted: impl Fn(&str) -> bool) -> (Vec<usize>, Vec<usize>) {
        let named: Vec<usize> = (0..self.jobs.len()).filter(|&id| !matches!(self.jobs[id].state, State::Ended(_)) && wanted(&self.jobs[id].asset)).collect();
        let (going, dropped): (Vec<usize>, Vec<usize>) = named.into_iter().partition(|&id| self.jobs[id].state == State::Going);
        dropped.iter().for_each(|&id| self.end(id, End::Dropped));
        (dropped, going)
    }

    /// Asset `from` is called `to` now: its jobs, what waits for it, and what each render tried of it.
    pub(crate) fn renamed(&mut self, from: &str, to: &str) {
        let (key, new) = (name_key(from), name_key(to));
        self.jobs.iter_mut().filter(|job| name_key(&job.asset) == key).for_each(|job| to.clone_into(&mut job.asset));
        if let Some(open) = self.open.remove(&key) {
            self.open.entry(new.clone()).or_default().extend(open);
        }
        for tried in self.tried.values_mut() {
            if tried.remove(&key) {
                tried.insert(new.clone());
            }
        }
    }

    /// Whether render `ask` tried `name` already: it isn't tried again.
    pub(crate) fn tried(&self, ask: &str, name: &str) -> bool {
        self.tried.get(ask).is_some_and(|tried| tried.contains(&name_key(name)))
    }

    fn make(&mut self, job: Job) -> usize {
        let id = self.jobs.len();
        self.by_key.insert(job.key.clone(), id);
        self.open.entry(name_key(&job.asset)).or_default().push(id);
        *self.left.entry(job.ask.clone()).or_default() += 1;
        self.ready.insert((job.rank, id));
        self.jobs.push(job);
        id
    }

    /// `id`'s needs are known: each hears of its end, and it stops being ready while it has any.
    fn linked(&mut self, id: usize) {
        let needs = self.jobs[id].needs.clone();
        let mut unmet = 0;
        for need in needs {
            if !matches!(self.jobs[need].state, State::Ended(_)) && need != id {
                self.jobs[need].readers.push(id);
                unmet += 1;
            }
        }
        self.jobs[id].unmet = unmet;
        if unmet > 0 {
            self.ready.remove(&(self.jobs[id].rank, id));
        }
    }

    fn wait_on(&mut self, id: usize, more: Vec<usize>) {
        let mut needs = std::mem::take(&mut self.jobs[id].needs);
        for need in more {
            if !needs.contains(&need) {
                needs.push(need);
            }
        }
        needs.retain(|&need| !matches!(self.jobs[need].state, State::Ended(_)));
        self.jobs[id].needs.clone_from(&needs);
        self.jobs[id].unmet = 0;
        for need in needs {
            if !self.jobs[need].readers.contains(&id) {
                self.jobs[need].readers.push(id);
            }
            self.jobs[id].unmet += 1;
        }
        if self.jobs[id].unmet == 0 {
            self.ready.insert((self.jobs[id].rank, id));
        }
    }

    /// A job for what node `i` needs to have a state, if render `ask` hasn't tried it: `spawned` is what now has to end first.
    fn owed_upstream(&mut self, i: usize, ask: &str, frozen: &mut dyn Frozen, spawned: &mut Vec<usize>) {
        if frozen.holds(i) {
            return;
        }
        let node = frozen.nodes()[i].clone();
        let key = name_key(&node.name);
        // On its way already, here or in another render.
        let making: Vec<usize> = (self.open.get(&key).into_iter().flatten()).copied().filter(|&id| self.jobs[id].ask == ask || self.jobs[id].spec == node.spec).collect();
        if !making.is_empty() {
            spawned.extend(making);
            return;
        }
        if self.tried(ask, &node.name) {
            return;
        }
        if node.makes && !node.free {
            let job_key = owed_key(ask, &key);
            if self.by_key.contains_key(&job_key) {
                return;
            }
            let rank = self.jobs.len() as u64 + 1;
            spawned.push(self.make(Job {
                key: job_key,
                ask: ask.to_owned(),
                node: i,
                asset: node.name.clone(),
                spec: node.spec.clone(),
                kind: Kind::Owed,
                state: State::Waiting,
                shot: node.shot,
                video: node.video,
                approve: false,
                rank,
                needs: vec![],
                unmet: 0,
                readers: vec![],
            }));
            return;
        }
        // No operation of its own: what it reads is what's owed.
        for &read in &node.reads {
            self.owed_upstream(read, ask, frozen, spawned);
        }
    }
}

/// What a render owes of the asset with this name key goes under one key, restart or not.
pub(crate) fn owed_key(ask: &str, name_key: &str) -> String {
    format!("project-{}", &hex::encode(Sha256::digest(format!("{ask}\n{name_key}")))[..32])
}

fn merged<'a>(lists: impl Iterator<Item = &'a Vec<usize>>) -> Vec<usize> {
    let mut all = vec![];
    let mut seen = HashSet::new();
    for &id in lists.flatten() {
        if seen.insert(id) {
            all.push(id);
        }
    }
    all
}
