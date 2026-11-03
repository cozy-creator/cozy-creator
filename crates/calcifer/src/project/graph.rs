//! What reads what, over any assets: names, `<mentions>`, and topological order.
use serde_json::Value;

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

/// Every `<…>` in any text under `value`.
pub fn mentions_in(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => mentions(text).into_iter().map(str::to_owned).collect(),
        Value::Array(items) => items.iter().flat_map(mentions_in).collect(),
        Value::Object(fields) => fields.values().flat_map(mentions_in).collect(),
        _ => vec![],
    }
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
    /// `fixed` (a revision's edge to its writer) goes last.
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
    while let Some(i) = (0..upstream.len()).find(|&i| !placed[i] && upstream[i].iter().all(|&edge| placed[edge])) {
        placed[i] = true;
        order.push(i);
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
