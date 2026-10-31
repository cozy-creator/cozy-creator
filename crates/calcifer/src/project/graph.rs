//! The project core, generic over project types: entities with versions, edges from `<Name>` mentions
//! and declared needs, states read and written in order (SSA), topological order and out of date.
//! A type lists its entities; nothing here knows what they depict.
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

/// One entity as its type lists it.
pub struct Entity {
    /// Unique by `name_key`.
    pub id: String,
    pub kind: &'static str,
    /// Whether `<id>` in a text names it.
    pub mentionable: bool,
    /// A state: its own version is revision 0; each ordered entity that writes it makes the next.
    pub state: bool,
    /// An ordered entity's position; states are read and written in this order.
    pub order: Option<usize>,
    /// Texts whose mentions are edges.
    pub texts: Vec<String>,
    /// Edges the type declares, by id.
    pub needs: Vec<String>,
    /// States it writes, by name.
    pub writes: Vec<String>,
    /// The chosen file (relative to the project) and its digest; None before the first version.
    pub file: Option<(String, String)>,
    pub versions: Vec<String>,
}

/// What an entity reads from another: its chosen file, or a file one of its versions wrote for a state.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edge {
    pub entity: usize,
    /// The state whose revision it reads from a writer; None reads the entity's chosen file.
    pub state: Option<String>,
}

pub struct Graph {
    pub entities: Vec<Entity>,
    pub upstream: Vec<Vec<Edge>>,
    /// Topological: every entity after what it reads.
    pub order: Vec<usize>,
    /// Mentions of no entity: the entity, and the mention as written.
    pub unknown: Vec<(usize, String)>,
}

/// Whether an entity's chosen file still follows from what it reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Current,
    /// No version yet.
    Empty,
    /// Out of date, and why.
    Stale(String),
}

impl Graph {
    /// Resolves the entities' edges and orders them; a duplicate name, an unknown need or a cycle is
    /// refused, naming it.
    pub fn new(entities: Vec<Entity>) -> Result<Self, Error> {
        let mut index = HashMap::new();
        for (i, entity) in entities.iter().enumerate() {
            if index.insert(name_key(&entity.id), i).is_some() {
                return Err(invalid("name_taken", format!("two entities are named {}", entity.id))
                    .remedy("rename one: names match in any case, with `_`, `-` and space alike"));
            }
        }
        let find = |name: &str| index.get(&name_key(name)).copied();
        let mut upstream = vec![];
        let mut unknown = vec![];
        for (i, entity) in entities.iter().enumerate() {
            let mut edges: Vec<Edge> = vec![];
            for need in &entity.needs {
                let j = find(need).ok_or_else(|| {
                    invalid("entity_unknown", format!("{} names {need}, which is no entity", entity.id))
                })?;
                edges.push(Edge { entity: j, state: None });
            }
            for written in &entity.writes {
                if !find(written).is_some_and(|s| entities[s].state) || entity.order.is_none() {
                    return Err(invalid(
                        "state_unknown",
                        format!("{} writes {written}, which is no state it can write", entity.id),
                    )
                    .remedy("`writes:` names world states, and only a shot writes them"));
                }
            }
            for mention in entity.texts.iter().flat_map(|text| mentions(text)) {
                match find(mention).filter(|&j| entities[j].mentionable && j != i) {
                    Some(s) if entities[s].state => edges.push(revision(&entities, &find, entity, s)),
                    Some(j) => edges.push(Edge { entity: j, state: None }),
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
        let order = topological(&entities, &upstream)?;
        Ok(Self { entities, upstream, order, unknown })
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        let key = name_key(name);
        self.entities.iter().position(|entity| name_key(&entity.id) == key)
    }

    /// The digest of what `edge` reads now, if it has been made.
    pub fn read(&self, lock: &Lock, edge: &Edge) -> Option<String> {
        let (_, digest) = self.entities[edge.entity].file.as_ref()?;
        match &edge.state {
            None => Some(digest.clone()),
            Some(state) => lock.made(&self.entities[edge.entity].id, digest)?.outputs.get(state).cloned(),
        }
    }

    /// Each entity's status. Only files make an entity out of date, never text: a made one is out of
    /// date when it read another file than its upstream holds now, or something it reads is out of
    /// date. A file attached by hand, and its clean, never is.
    pub fn status(&self, lock: &Lock) -> Vec<Status> {
        let mut status = vec![Status::Empty; self.entities.len()];
        for &i in &self.order {
            let entity = &self.entities[i];
            let Some((_, digest)) = &entity.file else {
                continue;
            };
            let Some(upstream) = lock.made(&entity.id, digest).and_then(|made| made.upstream.as_ref()) else {
                status[i] = Status::Current;
                continue;
            };
            let mut cause = None;
            for edge in &self.upstream[i] {
                let up = &self.entities[edge.entity].id;
                let read = self.read(lock, edge);
                cause = cause.or(match &status[edge.entity] {
                    Status::Empty => Some(format!("{up} has no version")),
                    Status::Stale(_) => Some(format!("{up} is out of date")),
                    Status::Current if read.is_none() => {
                        Some(format!("{up} wrote no {}", edge.state.as_deref().unwrap_or("file")))
                    }
                    Status::Current => (!upstream.iter().any(|u| {
                        name_key(&u.entity) == name_key(up) && u.state == edge.state && Some(&u.file) == read.as_ref()
                    }))
                    .then(|| format!("{up} changed")),
                });
            }
            status[i] = cause.map_or(Status::Current, Status::Stale);
        }
        status
    }

    /// Statuses by id, to say what an act put out of date.
    pub fn by_id(&self, status: &[Status]) -> BTreeMap<String, Status> {
        self.entities.iter().zip(status).map(|(entity, s)| (entity.id.clone(), s.clone())).collect()
    }
}

/// The revision of state `s` that `reader` reads: the latest write before it in order, else the
/// state's own (revision 0).
fn revision(entities: &[Entity], find: &impl Fn(&str) -> Option<usize>, reader: &Entity, s: usize) -> Edge {
    let before = |entity: &Entity| match (entity.order, reader.order) {
        (Some(written), Some(read)) => written < read,
        _ => false,
    };
    let writer = (entities.iter().enumerate())
        .filter(|(_, entity)| before(entity) && entity.writes.iter().any(|w| find(w) == Some(s)))
        .max_by_key(|(_, entity)| entity.order);
    match writer {
        Some((w, _)) => Edge { entity: w, state: Some(entities[s].id.clone()) },
        None => Edge { entity: s, state: None },
    }
}

/// Each entity after what it reads, else in the order the type listed it; what is left over holds a
/// cycle, which is refused naming it.
fn topological(entities: &[Entity], upstream: &[Vec<Edge>]) -> Result<Vec<usize>, Error> {
    let mut placed = vec![false; entities.len()];
    let mut order = vec![];
    while let Some(i) = (0..entities.len()).find(|&i| !placed[i] && upstream[i].iter().all(|edge| placed[edge.entity]))
    {
        placed[i] = true;
        order.push(i);
    }
    let Some(start) = (0..entities.len()).find(|&i| !placed[i]) else {
        return Ok(order);
    };
    let mut path = vec![start];
    loop {
        let at = *path.last().expect("a path");
        let up =
            upstream[at].iter().map(|edge| edge.entity).find(|&j| !placed[j]).expect("an entity left over reads one");
        if let Some(from) = path.iter().position(|&i| i == up) {
            // Each step of `path` reads the next, so reversed it runs the way the files flow.
            let mut cycle: Vec<&str> = path[from..].iter().rev().map(|&i| entities[i].id.as_str()).collect();
            cycle.push(&entities[at].id);
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
