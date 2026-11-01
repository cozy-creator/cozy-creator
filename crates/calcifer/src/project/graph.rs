//! The project core, generic over project types: nodes with versions, edges from `<Name>` mentions
//! and declared needs, states read and written in order (SSA), topological order and out of date.
//! A type lists its nodes; nothing here knows what they depict.
use std::collections::{BTreeMap, HashMap};

use protocol::error::{Error, Kind};

use super::folder::Lock;

/// How a name matches a mention: any case, with `_`, `-` and space alike (xs-135).
pub fn name_key(name: &str) -> String {
    name.replace(['_', '-'], " ").split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Every `<…>` in `text`, as written between its brackets, in order: no brackets or line break
/// inside.
pub fn mentions(text: &str) -> Vec<&str> {
    let mut found = vec![];
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        match after.find(['<', '>', '\n']) {
            Some(end) if end > 0 && after.as_bytes()[end] == b'>' => {
                found.push(&after[..end]);
                rest = &after[end + 1..];
            }
            Some(end) => rest = &after[end..],
            None => break,
        }
    }
    found
}

/// One node as its type lists it.
pub struct Node {
    /// Unique by `name_key`.
    pub id: String,
    pub kind: &'static str,
    /// What its alternatives are called: `take` (a shot's) or `version`.
    pub noun: &'static str,
    /// Whether `<id>` in a text names it.
    pub mentionable: bool,
    /// A state: its own version is revision 0; each ordered node that writes it makes the next.
    pub state: bool,
    /// An ordered node's position; states are read and written in this order.
    pub order: Option<usize>,
    /// Texts whose mentions are edges.
    pub texts: Vec<String>,
    /// Edges the type declares, by id.
    pub needs: Vec<String>,
    /// States it writes, by name.
    pub writes: Vec<String>,
    /// Files it reads that are no node's (a shot's references, a character's voice), by a key and
    /// their digest: a version made from another file is out of date.
    pub inputs: Vec<(String, String)>,
    /// The file it reads as now (relative to the project) and its digest: the approved version's
    /// primary asset, else the file attached in project.yaml (or that file's clean); None before the
    /// first version.
    pub file: Option<(String, String)>,
    /// Its versions' primary assets, in the order they were made.
    pub versions: Vec<String>,
}

/// What a node reads from another: its current file, or the frame one of its versions wrote for a state.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edge {
    pub node: usize,
    /// The state whose revision it reads from a writer; None reads the node's current file.
    pub state: Option<String>,
}

pub struct Graph {
    pub nodes: Vec<Node>,
    pub upstream: Vec<Vec<Edge>>,
    /// Topological: every node after what it reads.
    pub order: Vec<usize>,
    /// Mentions of no node: the node, and the mention as written.
    pub unknown: Vec<(usize, String)>,
}

/// Whether a node's current file still follows from what it reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Current,
    /// No version yet.
    Empty,
    /// Out of date, and why.
    Stale(String),
}

impl Graph {
    /// Resolves the nodes' edges and orders them; a duplicate name, an unknown need or a cycle is
    /// refused, naming it.
    pub fn new(nodes: Vec<Node>) -> Result<Self, Error> {
        let mut index = HashMap::new();
        for (i, node) in nodes.iter().enumerate() {
            if index.insert(name_key(&node.id), i).is_some() {
                return Err(invalid("name_taken", format!("two nodes are named {}", node.id))
                    .remedy("rename one: names match in any case, with `_`, `-` and space alike"));
            }
        }
        let find = |name: &str| index.get(&name_key(name)).copied();
        let mut upstream = vec![];
        let mut unknown = vec![];
        for (i, node) in nodes.iter().enumerate() {
            let mut edges: Vec<Edge> = vec![];
            for need in &node.needs {
                let j = find(need).ok_or_else(|| {
                    invalid("node_unknown", format!("{} names {need}, which is no node", node.id))
                })?;
                edges.push(Edge { node: j, state: None });
            }
            for written in &node.writes {
                if !find(written).is_some_and(|s| nodes[s].state) || node.order.is_none() {
                    return Err(invalid(
                        "state_unknown",
                        format!("{} writes {written}, which is no state it can write", node.id),
                    )
                    .remedy("`writes:` names world states, and only a shot writes them"));
                }
            }
            for mention in node.texts.iter().flat_map(|text| mentions(text)) {
                match find(mention).filter(|&j| nodes[j].mentionable && j != i) {
                    Some(s) if nodes[s].state => edges.push(revision(&nodes, &find, node, s)),
                    Some(j) => edges.push(Edge { node: j, state: None }),
                    None if find(mention) == Some(i) => {}
                    None => unknown.push((i, format!("<{mention}>"))),
                }
            }
            let mut seen = vec![];
            edges.retain(|edge| {
                !seen.contains(edge) && {
                    seen.push(edge.clone());
                    true
                }
            });
            upstream.push(edges);
        }
        let order = topological(&nodes, &upstream)?;
        Ok(Self { nodes, upstream, order, unknown })
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        let key = name_key(name);
        self.nodes.iter().position(|node| name_key(&node.id) == key)
    }

    /// What `edge` reads now, its path and sha256, if it has been made.
    pub fn read(&self, lock: &Lock, edge: &Edge) -> Option<(String, String)> {
        let (path, digest) = self.nodes[edge.node].file.as_ref()?;
        match &edge.state {
            None => Some((path.clone(), digest.clone())),
            Some(state) => {
                let frame = lock.made(&self.nodes[edge.node].id, path)?.writes.get(state)?;
                Some((frame.path.clone(), frame.sha256.clone()))
            }
        }
    }

    /// Each node's status. Only files make a node out of date, never text: a made one is out of
    /// date when it read another file than its upstream holds now, or something it reads is out of
    /// date. A file attached by hand, and its clean, never is.
    pub fn status(&self, lock: &Lock) -> Vec<Status> {
        let mut status = vec![Status::Empty; self.nodes.len()];
        for &i in &self.order {
            let node = &self.nodes[i];
            let Some((path, _)) = &node.file else {
                continue;
            };
            let Some(upstream) = lock.made(&node.id, path).and_then(|made| made.upstream.as_ref()) else {
                status[i] = Status::Current;
                continue;
            };
            let mut cause = None;
            for edge in &self.upstream[i] {
                let (up, noun) = (&self.nodes[edge.node].id, self.nodes[edge.node].noun);
                let read = self.read(lock, edge);
                cause = cause.or(match &status[edge.node] {
                    Status::Empty => Some(format!("{up} has no {noun}")),
                    Status::Stale(_) => Some(format!("{up} is out of date")),
                    Status::Current if read.is_none() => {
                        Some(format!("{up} wrote no {}", edge.state.as_deref().unwrap_or("file")))
                    }
                    Status::Current => (!upstream.iter().any(|u| {
                        name_key(&u.node) == name_key(up) && u.state == edge.state && Some(&u.asset) == read.as_ref().map(|(_, sha)| sha)
                    }))
                    .then(|| format!("{up} changed")),
                });
            }
            for (key, digest) in &node.inputs {
                let read = upstream.iter().any(|u| u.state.is_none() && &u.node == key && &u.asset == digest);
                cause = cause.or_else(|| (!read).then(|| format!("{key} changed")));
            }
            status[i] = cause.map_or(Status::Current, Status::Stale);
        }
        status
    }

    /// Statuses by id, to say what an act put out of date.
    pub fn by_id(&self, status: &[Status]) -> BTreeMap<String, Status> {
        self.nodes.iter().zip(status).map(|(node, s)| (node.id.clone(), s.clone())).collect()
    }
}

/// The revision of state `s` that `reader` reads: the latest write before it in order, else the
/// state's own (revision 0).
fn revision(nodes: &[Node], find: &impl Fn(&str) -> Option<usize>, reader: &Node, s: usize) -> Edge {
    let before = |node: &Node| match (node.order, reader.order) {
        (Some(written), Some(read)) => written < read,
        _ => false,
    };
    let writer = (nodes.iter().enumerate())
        .filter(|(_, node)| before(node) && node.writes.iter().any(|w| find(w) == Some(s)))
        .max_by_key(|(_, node)| node.order);
    match writer {
        Some((w, _)) => Edge { node: w, state: Some(nodes[s].id.clone()) },
        None => Edge { node: s, state: None },
    }
}

/// Each node after what it reads, else in the order the type listed it; what is left over holds a
/// cycle, which is refused naming it.
fn topological(nodes: &[Node], upstream: &[Vec<Edge>]) -> Result<Vec<usize>, Error> {
    let mut placed = vec![false; nodes.len()];
    let mut order = vec![];
    while let Some(i) = (0..nodes.len()).find(|&i| !placed[i] && upstream[i].iter().all(|edge| placed[edge.node]))
    {
        placed[i] = true;
        order.push(i);
    }
    let Some(start) = (0..nodes.len()).find(|&i| !placed[i]) else {
        return Ok(order);
    };
    let mut path = vec![start];
    loop {
        let at = *path.last().expect("a path");
        let up =
            upstream[at].iter().map(|edge| edge.node).find(|&j| !placed[j]).expect("a node left over reads one");
        if let Some(from) = path.iter().position(|&i| i == up) {
            // Each step of `path` reads the next, so reversed it runs the way the files flow.
            let mut cycle: Vec<&str> = path[from..].iter().rev().map(|&i| nodes[i].id.as_str()).collect();
            cycle.push(&nodes[at].id);
            return Err(invalid(
                "cycle",
                format!("a cycle: {} (each is made from the one before it)", cycle.join(" → ")),
            )
            .remedy("remove one <mention>, `setup:` or `frame:` that closes it"));
        }
        path.push(up);
    }
}

fn invalid(code: &str, message: String) -> Error {
    Error::new(Kind::Validation, code, message)
}
