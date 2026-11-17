//! The walker's graph (cl-770): one job per take a render names, made when it is asked and moved by what happens. Nothing is made on
//! another's behalf (cl-1192, Paul 11-18): a take reads what each box it reads has now, and waits only for a take of one with none that is
//! already on its way. Most jobs are one-offs running side by side, and assets come and go while they do. It reads a project only through
//! `Frozen`, so neither its format nor where its versions are kept reach this. Nothing here walks the project per event: an event touches
//! its job's readers, and a job's turn looks only at what it reads.
use std::collections::{BTreeSet, HashMap, HashSet};

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
    /// Calcifer makes it, free, with no machine, from what it reads has now.
    pub free: bool,
    /// Its state is a file the text names, and nothing moves it.
    pub pinned: bool,
    /// It makes a shot.
    pub shot: bool,
    /// What it makes is a video.
    pub video: bool,
}

/// What the walker reads of one frozen render, and what stands now.
pub(crate) trait Frozen {
    fn nodes(&self) -> &[Node];
    /// Every node, each after what it reads.
    fn order(&self) -> &[usize];
    /// Whether node `i` has a state now.
    fn holds(&mut self, i: usize) -> bool;
}

/// What a job is for: a new version asked of it by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A machine makes it.
    Take,
    /// Calcifer makes it.
    Free,
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
    /// Selected when it lands; existing asks retain their completed input snapshots.
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
    pub takes: Vec<Named>,
}

/// What a job's turn came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Turn {
    Go,
    /// It waits for a take on its way of something it reads that has none yet.
    Wait,
    /// Node `read`, which it reads, has nothing yet and nothing is making it: nothing makes it on this one's behalf.
    Unread { read: usize },
}

/// A project's jobs, all its renders' at once.
#[derive(Clone, Default)]
pub(crate) struct Graph {
    pub jobs: Vec<Job>,
    by_key: HashMap<String, usize>,
    /// Jobs not ended, by asset's name key.
    open: HashMap<String, Vec<usize>>,
    /// Waiting with nothing unmet, by rank.
    ready: BTreeSet<(u64, usize)>,
    /// Jobs not ended, by render.
    left: HashMap<String, usize>,
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

    /// Render `ask`: a job for each take it names, each waiting for this render's takes of what it reads. A box it doesn't name is read as
    /// it is: its approved version, else its newest.
    pub(crate) fn add(&mut self, ask: &str, frozen: &mut dyn Frozen, asked: &Asked) -> Vec<usize> {
        let nodes: Vec<Node> = frozen.nodes().to_vec();
        let order: Vec<usize> = frozen.order().to_vec();
        let by_name: HashMap<String, usize> = nodes.iter().enumerate().map(|(i, node)| (name_key(&node.name), i)).collect();
        let mut taken = vec![vec![]; nodes.len()];
        for take in &asked.takes {
            if let Some(&i) = by_name.get(&name_key(&take.asset)) {
                taken[i].push(take);
            }
        }
        // What a reader of each node waits for in this render: its takes here, unless it's approved and none of them approves. A node with
        // no operation (words, a file) passes on what it reads.
        let base = self.jobs.len() as u64 + 1;
        let mut through: Vec<Vec<usize>> = vec![vec![]; nodes.len()];
        let mut made = vec![];
        for (place, &i) in order.iter().enumerate() {
            let node = &nodes[i];
            let up = merged(node.reads.iter().map(|&read| &through[read]));
            let mut own = vec![];
            for take in &taken[i] {
                // Already going, as what a walker before this one sent.
                if self.by_key.contains_key(&take.key) {
                    continue;
                }
                own.push(self.make(Job {
                    key: take.key.clone(),
                    ask: ask.to_owned(),
                    node: i,
                    asset: node.name.clone(),
                    spec: node.spec.clone(),
                    kind: if node.free { Kind::Free } else { Kind::Take },
                    state: State::Waiting,
                    shot: node.shot,
                    video: node.video,
                    approve: take.approve,
                    rank: base + place as u64,
                    needs: up.clone(),
                    unmet: 0,
                    readers: vec![],
                }));
            }
            made.extend(own.iter().copied());
            // Going already, of this render, as what a walker before this one sent.
            let going = self.open.get(&name_key(&node.name)).into_iter().flatten().copied().filter(|&id| self.jobs[id].ask == ask && self.jobs[id].state == State::Going);
            own.extend(going);
            let approving = own.iter().any(|&id| self.jobs[id].approve);
            through[i] = match (own.is_empty(), node.pinned && !approving) {
                (true, _) if !node.makes => up,
                (true, _) | (false, true) => vec![],
                (false, false) => own,
            };
        }
        for &id in &made {
            self.linked(id);
        }
        made
    }

    /// A job's turn, looking only at what it reads: one with nothing yet is waited for while a take of it is on its way, here or in another
    /// render, and else said. Nothing is made of it on this one's behalf.
    pub(crate) fn turn(&mut self, id: usize, frozen: &mut dyn Frozen) -> Turn {
        let node = self.jobs[id].node;
        let (free, reads) = {
            let nodes = frozen.nodes();
            (nodes[node].free, nodes[node].reads.clone())
        };
        if free {
            return Turn::Go;
        }
        let mut making = vec![];
        let mut missing = None;
        for read in reads {
            if frozen.holds(read) {
                continue;
            }
            let key = name_key(&frozen.nodes()[read].name);
            let before = making.len();
            making.extend(self.open.get(&key).into_iter().flatten().copied().filter(|&other| other != id));
            if making.len() == before {
                missing.get_or_insert(read);
            }
        }
        if !making.is_empty() {
            self.wait_on(id, making);
            return Turn::Wait;
        }
        match missing {
            Some(read) => Turn::Unread { read },
            None => Turn::Go,
        }
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

    /// The jobs `take_ready` would take, left ready.
    pub(crate) fn ready(&self, room: usize) -> Vec<usize> {
        self.ready.iter().take(room).map(|(_, id)| *id).collect()
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
        for reader in std::mem::take(&mut self.jobs[id].readers) {
            let job = &mut self.jobs[reader];
            job.unmet = job.unmet.saturating_sub(1);
            if job.unmet == 0 && job.state == State::Waiting {
                self.ready.insert((job.rank, reader));
            }
        }
    }

    /// Those waiting that `wanted` names leave, and what reads them finds it at its turn. Answers them, and those going, whose runs are the caller's to cancel.
    pub(crate) fn cancel(&mut self, wanted: impl Fn(&str, &str) -> bool) -> (Vec<usize>, Vec<usize>) {
        let named: Vec<usize> = (0..self.jobs.len()).filter(|&id| !matches!(self.jobs[id].state, State::Ended(_)) && wanted(&self.jobs[id].asset, &self.jobs[id].key)).collect();
        let (going, dropped): (Vec<usize>, Vec<usize>) = named.into_iter().partition(|&id| self.jobs[id].state == State::Going);
        dropped.iter().for_each(|&id| self.end(id, End::Dropped));
        (dropped, going)
    }

    /// Asset `from` is called `to` now: its jobs, and what waits for it.
    pub(crate) fn renamed(&mut self, from: &str, to: &str) {
        let (key, new) = (name_key(from), name_key(to));
        self.jobs.iter_mut().filter(|job| name_key(&job.asset) == key).for_each(|job| to.clone_into(&mut job.asset));
        if let Some(open) = self.open.remove(&key) {
            self.open.entry(new).or_default().extend(open);
        }
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
}

/// What a take's run key starts with: `project-` and 32 hex digits, whoever made it.
const OWED: &str = "project-";

/// Whether a run's key is a walk's: the run of a take.
pub(crate) fn is_owed_key(key: &str) -> bool {
    key.starts_with(OWED)
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
