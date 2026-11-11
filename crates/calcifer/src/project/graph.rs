//! What reads what, over any assets: names, `<mentions>`, and topological order.
use std::{cmp::Reverse, collections::BinaryHeap};

use serde_json::Value;

/// How a name matches a mention: any case, with `_` and `-` alike. Space folds with them, so a lock from before names lost theirs
/// still finds its versions.
pub(crate) fn name_key(name: &str) -> String {
    name.replace(['_', '-'], " ").split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn in_a_name(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-')
}

/// Whether `text` can be a name: only letters, digits, `_` and `-`. A `.` selects inside a box, and `:` a part of its video.
pub(crate) fn is_name(text: &str) -> bool {
    !text.is_empty() && text.chars().all(in_a_name)
}

/// What a box calls itself in its own text: `<self.2>`, a version of it.
pub(crate) const SELF: &str = "self";

/// A mention, `<name.2.cleaned:48-120>`: a box, a version of it, an extra of that version, and a part of its video.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mention<'a> {
    pub name: &'a str,
    pub version: Option<u32>,
    /// An extra output of the version: `<page.cleaned>`.
    pub part: Option<&'a str>,
    pub span: Option<Span>,
}

/// What a mention names of a video, after its `:`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Span {
    /// Frames `in` to `out` (not counting it), or to its end: `:48-120`, `:48-`.
    Frames(u32, Option<u32>),
    /// An orbit's frame at this angle, as a picture: `:120deg`.
    Angle(f64),
    /// An orbit's turn from one angle to another, as a clip: `:90deg-180deg`.
    Turn(f64, f64),
}

impl Mention<'_> {
    /// How a function hears it, its box's own name in place of how it was typed.
    pub(crate) fn spelled(&self, name: &str) -> String {
        let mut out = name.to_owned();
        if let Some(version) = self.version {
            out.push_str(&format!(".{version}"));
        }
        if let Some(part) = self.part {
            out.push_str(&format!(".{part}"));
        }
        match self.span {
            None => {}
            Some(Span::Frames(from, to)) => out.push_str(&format!(":{from}-{}", to.map_or_else(String::new, |to| to.to_string()))),
            Some(Span::Angle(at)) => out.push_str(&format!(":{at}deg")),
            Some(Span::Turn(from, to)) => out.push_str(&format!(":{from}deg-{to}deg")),
        }
        out
    }
}

/// `inner` as a mention, if it is one.
pub(crate) fn mention(inner: &str) -> Option<Mention<'_>> {
    let (head, span) = match inner.split_once(':') {
        None => (inner, None),
        Some((head, span)) => (head, Some(span_of(span)?)),
    };
    let mut parts = head.split('.');
    let name = parts.next().filter(|name| is_name(name))?;
    let (mut version, mut part) = (None, None);
    for (n, word) in parts.enumerate() {
        match word.parse::<u32>() {
            Ok(number) if n == 0 && word.bytes().all(|byte| byte.is_ascii_digit()) => version = Some(number),
            _ if part.is_none() && is_name(word) => part = Some(word),
            _ => return None,
        }
    }
    Some(Mention { name, version, part, span })
}

/// `48-120`, `48-`, `120deg` or `90deg-180deg`.
fn span_of(text: &str) -> Option<Span> {
    let number = |text: &str| text.bytes().all(|byte| byte.is_ascii_digit()).then(|| text.parse::<u32>().ok()).flatten();
    let degrees = |text: &str| {
        let digits = text.strip_suffix("deg")?;
        let (whole, fraction) = digits.split_once('.').unwrap_or((digits, "0"));
        let plain = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
        (plain(whole) && plain(fraction)).then(|| digits.parse::<f64>().ok()).flatten().filter(|at| at.is_finite())
    };
    match text.split_once('-') {
        None => Some(Span::Angle(degrees(text)?)),
        Some((from, to)) if from.ends_with("deg") => Some(Span::Turn(degrees(from)?, degrees(to)?)),
        Some((from, to)) => Some(Span::Frames(number(from)?, if to.is_empty() { None } else { Some(number(to)?) })),
    }
}

/// `abr.2` as the name it's a version of, and that number.
pub(crate) fn version_of(mention: &str) -> Option<(&str, u32)> {
    let read = self::mention(mention)?;
    Some((read.name, read.version?))
}

/// Whether something new can go by `name`: no `.`, and `self` is the box a text is in.
pub(crate) fn can_take(name: &str) -> bool {
    is_name(name) && name_key(name) != SELF
}

/// `mention` once `from` is called `to`, what follows its name with it: `<from>`, `<from.2>`, `<from:4-9>`. None: it doesn't say `from`.
pub(crate) fn mention_moved(mention: &str, from: &str, to: &str) -> Option<String> {
    let read = self::mention(mention)?;
    (name_key(read.name) == name_key(from)).then(|| format!("{to}{}", &mention[read.name.len()..]))
}

/// `text` as it could be a name: what a name can't hold becomes `_`.
pub(crate) fn as_name(text: &str) -> String {
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
pub(crate) fn mentions_replaced(text: &str, with: &mut dyn FnMut(&str) -> Option<String>) -> String {
    let (mut said, mut rest) = (String::with_capacity(text.len()), text);
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let inner = after.find(['<', '>', '\n']).filter(|&end| after.as_bytes()[end] == b'>').map(|end| &after[..end]);
        said.push_str(&rest[..open]);
        match inner.filter(|inner| mention(inner).is_some()).and_then(|inner| Some((inner, with(inner)?))) {
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

/// Every mention in `text`: a `<…>` holding a name, and what selects in it.
pub(crate) fn mentions(text: &str) -> Vec<&str> {
    bracketed(text).into_iter().filter(|text| mention(text).is_some()).collect()
}

/// Every mention in any text under `value`.
pub(crate) fn mentions_in(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => mentions(text).into_iter().map(str::to_owned).collect(),
        Value::Array(items) => items.iter().flat_map(mentions_in).collect(),
        Value::Object(fields) => fields.values().flat_map(mentions_in).collect(),
        _ => vec![],
    }
}

/// How many single-character edits turn `a` into `b`.
pub(crate) fn edits(a: &str, b: &str) -> usize {
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

pub(crate) struct Graph {
    pub upstream: Vec<Vec<usize>>,
    /// Topological: every asset after what it reads.
    pub order: Vec<usize>,
    /// Mentions that would close a cycle, left out: the reader, and what it mentions.
    pub closing: Vec<(usize, usize)>,
}

impl Graph {
    /// Each asset's reads ordered; a mention that closes a cycle is left out, the text staying as written (`closing`), and
    /// `fixed` (an entity's edge to what it links) goes last.
    pub(crate) fn new(mut upstream: Vec<Vec<usize>>, fixed: &[Vec<usize>]) -> Self {
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
