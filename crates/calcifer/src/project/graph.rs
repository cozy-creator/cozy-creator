//! What reads what, over any assets: names, `<mentions>`, and topological order.
use std::{cmp::Reverse, collections::BinaryHeap};

use serde_json::Value;

/// How a name matches a mention: any case, with `_` and `-` alike. Space folds with them, so a lock from before names lost theirs
/// still finds its versions.
pub fn name_key(name: &str) -> String {
    name.replace(['_', '-'], " ").split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn in_a_name(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// Whether `text` can be a name: only letters, digits, `_`, `-` and `.`.
pub fn is_name(text: &str) -> bool {
    !text.is_empty() && text.chars().all(in_a_name)
}

/// `text` as it could be a name: what a name can't hold becomes `_`.
pub fn as_name(text: &str) -> String {
    let parts = text.split(|c| !in_a_name(c)).filter(|part| !part.is_empty());
    parts.collect::<Vec<_>>().join("_")
}

/// Every `<…>` in `text`, as written between its brackets, in order: no brackets or line break inside.
fn bracketed(text: &str) -> Vec<&str> {
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

/// `text` with each mention `with` has words for replaced by them, brackets and all. The rest stays as written.
pub fn mentions_replaced(text: &str, with: &mut dyn FnMut(&str) -> Option<String>) -> String {
    let (mut said, mut rest) = (String::with_capacity(text.len()), text);
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let inner = after.find(['<', '>', '\n']).filter(|&end| after.as_bytes()[end] == b'>').map(|end| &after[..end]);
        said.push_str(&rest[..open]);
        match inner.filter(|inner| is_name(inner)).and_then(|inner| Some((inner, with(inner)?))) {
            Some((inner, words)) => {
                said.push_str(&words);
                rest = &after[inner.len() + 1..];
            }
            None => {
                said.push('<');
                rest = after;
            }
        }
    }
    said.push_str(rest);
    said
}

/// Every mention in `text`: a `<…>` holding a name.
pub fn mentions(text: &str) -> Vec<&str> {
    bracketed(text).into_iter().filter(|text| is_name(text)).collect()
}

/// Every mention in any text under `value`.
pub fn mentions_in(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => mentions(text).into_iter().map(str::to_owned).collect(),
        Value::Array(items) => items.iter().flat_map(mentions_in).collect(),
        Value::Object(fields) => fields.values().flat_map(mentions_in).collect(),
        _ => vec![],
    }
}

/// How many single-character edits turn `a` into `b`.
pub fn edits(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let cost = if x == y { diagonal } else { diagonal.min(row[j]).min(row[j + 1]) + 1 };
            diagonal = row[j + 1];
            row[j + 1] = cost;
        }
    }
    row[b.len()]
}

pub struct Graph {
    pub upstream: Vec<Vec<usize>>,
    /// Topological: every asset after what it reads.
    pub order: Vec<usize>,
    /// Mentions that would close a cycle, left out: the reader, and what it mentions.
    pub closing: Vec<(usize, usize)>,
}

impl Graph {
    /// Each asset's reads ordered; a mention that closes a cycle is left out, the text staying as written (`closing`), and
    /// `fixed` (an entity's edge to what it links) goes last.
    pub fn new(mut upstream: Vec<Vec<usize>>, fixed: &[Vec<usize>]) -> Self {
        let mut closing = vec![];
        let order = loop {
            let cycle = match topological(&upstream) {
                Ok(order) => break order,
                Err(cycle) => cycle,
            };
            // The step that closes it first, among those made only of mentions.
            let steps: Vec<(usize, usize)> = (0..cycle.len()).rev().map(|n| (cycle[n], cycle[(n + 1) % cycle.len()])).collect();
            let (reader, read) = steps.iter().copied().find(|(reader, read)| !fixed[*reader].contains(read)).unwrap_or(steps[0]);
            upstream[reader].retain(|edge| *edge != read);
            closing.push((reader, read));
        };
        Self { upstream, order, closing }
    }
}

/// Each asset after what it reads, else in file order; what is left over holds a cycle, returned as
/// its assets, each reading the next and the last the first.
fn topological(upstream: &[Vec<usize>]) -> Result<Vec<usize>, Vec<usize>> {
    let mut placed = vec![false; upstream.len()];
    let mut order = vec![];
    let mut readers = vec![vec![]; upstream.len()];
    let mut waits: Vec<usize> = upstream.iter().map(Vec::len).collect();
    for (i, reads) in upstream.iter().enumerate() {
        for &read in reads {
            readers[read].push(i);
        }
    }
    let mut ready: BinaryHeap<Reverse<usize>> = (0..upstream.len()).filter(|&i| waits[i] == 0).map(Reverse).collect();
    while let Some(Reverse(i)) = ready.pop() {
        placed[i] = true;
        order.push(i);
        for &reader in &readers[i] {
            waits[reader] -= 1;
            if waits[reader] == 0 {
                ready.push(Reverse(reader));
            }
        }
    }
    let Some(start) = (0..upstream.len()).find(|&i| !placed[i]) else {
        return Ok(order);
    };
    let mut path = vec![start];
    loop {
        let at = *path.last().expect("a path");
        let up = upstream[at].iter().copied().find(|&j| !placed[j]).expect("an asset left over reads one");
        if let Some(from) = path.iter().position(|&i| i == up) {
            return Err(path[from..].to_vec());
        }
        path.push(up);
    }
}
