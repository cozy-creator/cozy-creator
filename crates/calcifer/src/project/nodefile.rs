//! A node as project.yaml holds it: the one place that knows where a node's fields land in the file, so the routes and the page speak in
//! nodes and the file can change under them. Each write finds the node's lines, changes only those, keeps comments and `|`, and checks
//! the file reads as the same project with that one change.
use std::{collections::HashMap, ops::Range};

use protocol::{
    error::{Error, Kind},
    yaml::to_json_keeping,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    folder::PROJECT,
    format::{Place, canonical, field_rest, invalid, key_of, names_a_file, significant, slot, spaces, split_comment, under_section, yaml_key},
    graph::{mentions, name_key},
};

/// Where a node's tool lands.
pub(super) const TOOL: &str = "with";
/// The fields that are how it's made. Any other lands at the root of its lines, unless it's already written in `render`.
const IN_RENDER: [&str; 8] = ["with", "prompt", "models", "lora", "aspect_ratio", "megapixels", "turbo", "style"];
const PINS: [&str; 3] = ["image", "video", "audio"];
const SECTIONS: [&str; 2] = ["assets", "entities"];

fn by_hand(name: &str, why: impl std::fmt::Display) -> Error {
    invalid("project_edit", format!("{name} can't be written in {PROJECT}: {why}")).next(format!("edit {name} in {PROJECT} yourself"))
}

fn read(text: &str) -> Result<Value, Error> {
    to_json_keeping(text, names_a_file).map_err(|why| invalid("project_yaml", format!("{PROJECT}: {why}")))
}

fn pad(indent: usize) -> String {
    " ".repeat(indent)
}

/// Where a node's own lines end: a blank line or a comment after them, at or left of its name, is the next one's.
fn end_of(lines: &[&str], key: usize, finish: usize) -> usize {
    let at = spaces(lines[key]);
    ((key + 1)..finish).rev().find(|&n| significant(lines[n]) || (!lines[n].trim().is_empty() && spaces(lines[n]) > at)).map_or(key + 1, |n| n + 1)
}

fn digest(lines: &[&str]) -> String {
    format!("\"{}\"", hex::encode(Sha256::digest(lines.concat())))
}

/// The digest of node `name`'s own lines, so an edit of another moves nothing.
pub(super) fn etag(text: &str, name: &str) -> Option<String> {
    let place = Place::find(text, name)?;
    Some(digest(&place.lines[place.key..end_of(&place.lines, place.key, place.body.end)]))
}

/// Every node's ETag, by name key, at once: a read of the project needn't look for each.
pub(super) fn etags(text: &str) -> HashMap<String, String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut found = HashMap::new();
    for section in SECTIONS {
        let Some(header) = lines.iter().position(|line| spaces(line) == 0 && key_of(line).as_deref() == Some(section)) else { continue };
        let end = ((header + 1)..lines.len()).find(|&n| significant(lines[n]) && spaces(lines[n]) == 0).unwrap_or(lines.len());
        let Some(at) = lines[header + 1..end].iter().find(|line| significant(line)).map(|line| spaces(line)) else { continue };
        let keys: Vec<usize> = ((header + 1)..end).filter(|&n| significant(lines[n]) && spaces(lines[n]) == at).collect();
        for (n, &key) in keys.iter().enumerate() {
            let finish = keys.get(n + 1).copied().unwrap_or(end);
            if let Some(name) = key_of(lines[key]) {
                found.insert(name_key(&name), digest(&lines[key..end_of(&lines, key, finish)]));
            }
        }
    }
    found
}

/// A value on one line: plain where YAML reads it back as this text, else quoted.
fn inline(value: &Value) -> String {
    match value {
        Value::String(text) if !text.contains(':') && to_json_keeping(text, |_| false).is_ok_and(|read| read == *value) => text.clone(),
        _ => value.to_string(),
    }
}

/// Words go as `|` text. Anything YAML couldn't hold in that goes quoted.
fn wordy(text: &str) -> bool {
    text.trim().contains([' ', '\n']) && !text.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
}

fn text_lines(text: &str, indent: usize) -> Vec<String> {
    text.trim().split('\n').map(|line| if line.is_empty() { "\n".to_owned() } else { format!("{}{line}\n", pad(indent)) }).collect()
}

/// `field: value` as lines of its own at `indent`.
fn emitted(field: &str, value: &Value, indent: usize) -> Vec<String> {
    let head = format!("{}{}:", pad(indent), yaml_key(field));
    match value {
        Value::Array(items) if !items.is_empty() => std::iter::once(format!("{head}\n")).chain(items.iter().flat_map(|item| item_lines(item, indent + 2))).collect(),
        Value::Object(fields) if !fields.is_empty() => {
            std::iter::once(format!("{head}\n")).chain(fields.iter().flat_map(|(field, value)| emitted(field, value, indent + 2))).collect()
        }
        Value::String(text) if wordy(text) => std::iter::once(format!("{head} |\n")).chain(text_lines(text, indent + 2)).collect(),
        value => vec![format!("{head} {}\n", inline(value))],
    }
}

/// A list's item at `indent`: a mapping's first field on the dash's line, a beat's `duration` and a write's `image` first.
fn item_lines(item: &Value, indent: usize) -> Vec<String> {
    match item {
        Value::Object(fields) if !fields.is_empty() => {
            let (first, rest): (Vec<_>, Vec<_>) = fields.iter().partition(|(field, _)| ["duration", "image"].contains(&field.as_str()));
            let mut lines: Vec<String> = first.into_iter().chain(rest).flat_map(|(field, value)| emitted(field, value, indent + 2)).collect();
            lines[0] = format!("{}- {}", pad(indent), &lines[0][indent + 2..]);
            lines
        }
        Value::String(text) if wordy(text) => std::iter::once(format!("{}- |\n", pad(indent))).chain(text_lines(text, indent + 2)).collect(),
        item => vec![format!("{}- {}\n", pad(indent), inline(item))],
    }
}

/// The parts of a mapping all on one line, as lines of their own: a comma inside quotes or brackets is no separator.
fn flow_lines(inner: &str, indent: usize) -> Vec<String> {
    let (mut parts, mut start, mut depth, mut quote) = (vec![], 0, 0, None);
    for (at, c) in inner.char_indices() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '{' | '[') => depth += 1,
            (None, '}' | ']') => depth -= 1,
            (None, ',') if depth == 0 => {
                parts.push(&inner[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    parts.into_iter().map(str::trim).filter(|part| !part.is_empty()).map(|part| format!("{}{part}\n", pad(indent))).collect()
}

/// The lines `field` fills among a mapping at `indent`: past a list written at its key's own indent, and up to a comment or blank line that isn't its.
fn span_of(lines: &[String], within: &Range<usize>, indent: usize, field: &str) -> Option<Range<usize>> {
    let at = within.clone().find(|&n| significant(&lines[n]) && spaces(&lines[n]) == indent && key_of(&lines[n]).as_deref() == Some(field))?;
    let item = |line: &str| line.trim_start().starts_with("- ") || line.trim() == "-";
    let end = ((at + 1)..within.end).find(|&n| significant(&lines[n]) && (spaces(&lines[n]) < indent || (spaces(&lines[n]) == indent && !item(&lines[n])))).unwrap_or(within.end);
    let last = ((at + 1)..end).rev().find(|&n| significant(&lines[n]) || (!lines[n].trim().is_empty() && spaces(&lines[n]) > indent));
    Some(at..last.map_or(at + 1, |n| n + 1))
}

/// Whether what a field's lines write holds a comment that replacing them loses. Its own line's stays, and a `#` in a `|` text is its text.
fn holds_comment(lines: &[String]) -> bool {
    let mut text_under = split_comment(lines[0].trim()).0.ends_with(['|', '>']).then(|| spaces(&lines[0]));
    for line in lines.iter().skip(1) {
        if text_under.is_some_and(|indent| spaces(line) > indent || line.trim().is_empty()) {
            continue;
        }
        text_under = None;
        let (written, comment) = split_comment(line.trim());
        if comment.is_some() {
            return true;
        }
        if written.ends_with(['|', '>']) {
            text_under = Some(spaces(line));
        }
    }
    false
}

/// `render`'s lines as a mapping, whatever form they're in: its text becomes `prompt`. What it fills now, and how far its fields are indented.
fn open_render(lines: &mut Vec<String>, span: Range<usize>, indent: usize) -> (Range<usize>, usize) {
    let (written, comment) = {
        let (written, comment) = split_comment(field_rest(&lines[span.start], "render").1);
        (written.to_owned(), comment.map_or(String::new(), |comment| format!("  {comment}")))
    };
    if written.is_empty() {
        let inner = ((span.start + 1)..span.end).find(|&n| significant(&lines[n])).map_or(indent + 2, |n| spaces(&lines[n]));
        return ((span.start + 1)..span.end, inner);
    }
    let fields = match written.strip_prefix('{').and_then(|inner| inner.strip_suffix('}')) {
        Some(inner) => flow_lines(inner, indent + 2),
        None => {
            let rest = lines[span.start + 1..span.end].iter().map(|line| if line.trim().is_empty() { line.clone() } else { format!("  {line}") });
            std::iter::once(format!("{}prompt: {written}\n", pad(indent + 2))).chain(rest).collect()
        }
    };
    let count = fields.len();
    lines.splice(span.clone(), std::iter::once(format!("{}render:{comment}\n", pad(indent))).chain(fields));
    ((span.start + 1)..(span.start + 1 + count), indent + 2)
}

/// `field` set in what the file reads as: with `render` in the form the edit leaves it.
fn expect(wanted: &mut Value, name: &str, in_render: bool, field: &str, value: &Value) -> Result<(), String> {
    let asset = slot(wanted, name).ok_or("it isn't in the file")?;
    if asset.is_null() {
        *asset = json!({});
    }
    let asset = asset.as_object_mut().ok_or("it is no mapping of fields")?;
    if !in_render {
        match value {
            Value::Null => drop(asset.remove(field)),
            value => drop(asset.insert(field.to_owned(), value.clone())),
        }
        return Ok(());
    }
    let render = asset.entry("render").or_insert(Value::Null);
    match render.take() {
        Value::Null => *render = json!({}),
        text @ Value::String(_) => *render = json!({"prompt": text}),
        held => *render = held,
    }
    let target = render.as_object_mut().ok_or("its render is no mapping of fields")?;
    match value {
        Value::Null => drop(target.remove(field)),
        value => drop(target.insert(field.to_owned(), value.clone())),
    }
    if target.is_empty() {
        asset.remove("render");
    }
    Ok(())
}

/// What the file says of a project, with what an empty section or node reads as made the same.
fn tidied(mut value: Value, name: &str) -> Value {
    if let Some(asset) = slot(&mut value, name).filter(|asset| asset.is_null()) {
        *asset = json!({});
    }
    for section in SECTIONS {
        if value.get(section).is_some_and(|held| held.is_null() || held.as_object().is_some_and(Map::is_empty)) {
            value.as_object_mut().expect("a section's mapping").remove(section);
        }
    }
    canonical(&value)
}

/// `text` with one field of node `name` written: its lines replaced, or new ones after what's there. Whether a comment inside them went.
fn write_field(text: &str, name: &str, in_render: bool, field: &str, value: &Value) -> Result<(String, bool), Error> {
    let place = Place::find(text, name).ok_or_else(|| by_hand(name, "it has no lines of its own"))?;
    let mut lines: Vec<String> = place.lines.iter().map(|line| if line.ends_with('\n') { (*line).to_owned() } else { format!("{line}\n") }).collect();
    let (key, mut indent) = (place.key, place.child);
    let mut body = (key + 1)..end_of(&place.lines, key, place.body.end);
    if let Some((head, inner, tail)) = &place.flow {
        let fields = flow_lines(inner, spaces(&lines[key]) + 2);
        indent = spaces(&lines[key]) + 2;
        lines[key] = format!("{}{}\n", head.trim_end(), tail.trim_end());
        let count = fields.len();
        lines.splice((key + 1)..(key + 1), fields);
        body = (key + 1)..(key + 1 + count);
    }
    let mapping = match (in_render, span_of(&lines, &body, indent, "render")) {
        (false, _) => (body, indent),
        (true, Some(span)) => open_render(&mut lines, span, indent),
        (true, None) if value.is_null() => return Ok((lines.concat(), false)),
        (true, None) => {
            lines.insert(body.end, format!("{}render:\n", pad(indent)));
            ((body.end + 1)..(body.end + 1), indent + 2)
        }
    };
    let (within, indent) = mapping;
    let held = span_of(&lines, &within, indent, field);
    let lost = held.as_ref().is_some_and(|span| !value.is_null() && holds_comment(&lines[span.clone()]));
    match (held, value) {
        (None, Value::Null) => {}
        (Some(span), Value::Null) => {
            let gone = span.len();
            lines.drain(span);
            let left = within.start..(within.end - gone);
            if in_render && !lines[left.clone()].iter().any(|line| significant(line)) {
                lines.drain((left.start - 1)..left.end);
            }
        }
        (Some(span), value) => {
            let comment = split_comment(field_rest(&lines[span.start], field).1).1.map(str::to_owned);
            let mut fresh = emitted(field, value, indent);
            if let Some(comment) = comment {
                fresh[0] = format!("{}  {comment}\n", fresh[0].trim_end_matches('\n'));
            }
            lines.splice(span, fresh);
        }
        (None, value) => drop(lines.splice(within.end..within.end, emitted(field, value, indent))),
    }
    Ok((lines.concat(), lost))
}

/// `text` with these fields of node `name` set, a null cleared, and what went doing so. Setting what it holds changes nothing.
pub(super) fn set(text: &str, name: &str, fields: &Map<String, Value>) -> Result<(String, Vec<String>), Error> {
    let mut wanted = read(text)?;
    let (mut edited, mut warnings) = (text.to_owned(), vec![]);
    for (field, value) in fields {
        let asset = slot(&mut wanted, name).ok_or_else(|| by_hand(name, "it has no lines of its own"))?.clone();
        let rendered = asset.get("render").and_then(Value::as_object).is_some_and(|render| render.contains_key(field));
        let in_render = asset.get(field).is_none() && (rendered || IN_RENDER.contains(&field.as_str()));
        if PINS.contains(&field.as_str()) && asset.get(field).and_then(Value::as_str).is_some_and(|held| mentions(held).is_empty()) {
            let said = format!("{name} is accepted as that {field}: unaccept it to change what it reads");
            return Err(Error::new(Kind::Usage, "node_accepted", said));
        }
        let held = match in_render {
            true => asset.get("render").and_then(|render| render.get(field)),
            false => asset.get(field),
        };
        if held.map_or(value.is_null(), |held| canonical(held) == canonical(value)) {
            continue;
        }
        expect(&mut wanted, name, in_render, field, value).map_err(|why| by_hand(name, why))?;
        let (next, lost) = write_field(&edited, name, in_render, field, value)?;
        edited = next;
        if lost {
            warnings.push(format!("a comment inside {name}'s {field} was lost"));
        }
    }
    (tidied(read(&edited)?, name) == tidied(wanted, name)).then_some(()).ok_or_else(|| by_hand(name, "the edit would change something else"))?;
    Ok((edited, warnings))
}

/// `text` with a new node at the end of `assets:`, made by `tool`.
pub(super) fn added(text: &str, name: &str, tool: &str, kind: &str, fields: &Map<String, Value>) -> Result<(String, Vec<String>), Error> {
    let block = format!("{}:\n  kind: {kind}\n  render:\n    with: {}\n", yaml_key(name), inline(&json!(tool)));
    let edited = add_block(text, "assets", name, &block)?;
    set(&edited, name, fields)
}

fn add_block(text: &str, section: &str, name: &str, block: &str) -> Result<String, Error> {
    let mut wanted = read(text)?;
    if wanted.is_null() {
        wanted = json!({});
    }
    // In its section, where a hash reads as a hash.
    let framed: String = block.split_inclusive('\n').map(|line| format!("  {line}")).collect();
    let framed = read(&format!("{section}:\n{framed}")).map_err(|_| by_hand(name, "its lines don't read"))?;
    let top = wanted.as_object_mut().ok_or_else(|| by_hand(name, "the file is no mapping of sections"))?;
    let held = top.entry(section).or_insert(Value::Null);
    if held.is_null() {
        *held = json!({});
    }
    let body = framed[section].as_object().and_then(|block| block.values().next()).cloned().unwrap_or(Value::Null);
    held.as_object_mut().ok_or_else(|| by_hand(name, format!("its `{section}:` is no mapping")))?.insert(name.to_owned(), body);
    let wanted = canonical(&wanted);
    let same = |edited: &String| read(edited).is_ok_and(|read| canonical(&read) == wanted);
    under_section(text, section, block).into_iter().find(same).ok_or_else(|| by_hand(name, "the edit would change something else"))
}

/// `text` with node `name`'s lines gone, and what puts them back.
pub(super) fn removed(text: &str, name: &str) -> Result<(String, String), Error> {
    let place = Place::find(text, name).ok_or_else(|| by_hand(name, "it has no lines of its own"))?;
    let at = spaces(place.lines[place.key]);
    let section = SECTIONS.into_iter().find(|section| {
        let header = place.lines[..place.key].iter().rposition(|line| significant(line) && spaces(line) == 0);
        header.is_some_and(|header| key_of(place.lines[header]).as_deref() == Some(section))
    });
    let Some(section) = section.filter(|_| at > 0) else { return Err(by_hand(name, "it is part of what's around it")) };
    let end = end_of(&place.lines, place.key, place.body.end);
    let back = format!("{section}:\n{}", place.lines[place.key..end].concat());
    let edited = [place.lines[..place.key].concat(), place.lines[end..].concat()].concat();
    let mut wanted = read(text)?;
    let key = wanted[section].as_object().and_then(|held| held.keys().find(|key| name_key(key) == name_key(name)).cloned());
    let key = key.ok_or_else(|| by_hand(name, "it is part of what's around it"))?;
    wanted[section].as_object_mut().expect("a section's mapping").remove(&key);
    (tidied(read(&edited)?, name) == tidied(wanted, name)).then_some(()).ok_or_else(|| by_hand(name, "the edit would change something else"))?;
    Ok((edited, back))
}

/// `text` with what `removed` said back in its section, under the name it had.
pub(super) fn restored(text: &str, name: &str, back: &str) -> Result<String, Error> {
    let refused = || Error::new(Kind::Validation, "node_text", format!("that isn't what deleting {name} answered"));
    let (header, lines) = back.split_once('\n').ok_or_else(refused)?;
    let section = SECTIONS.into_iter().find(|section| header.strip_suffix(':') == Some(*section)).ok_or_else(refused)?;
    let at = lines.lines().next().map_or(0, spaces);
    if lines.lines().next().and_then(key_of).is_none_or(|key| name_key(&key) != name_key(name)) || read(back).is_err() {
        return Err(refused());
    }
    let block: String = lines.split_inclusive('\n').map(|line| line.get(at..).filter(|_| spaces(line) >= at).unwrap_or(line)).collect();
    add_block(text, section, name, &block)
}
