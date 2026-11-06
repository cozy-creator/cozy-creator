//! An asset as project.yaml holds it: the one place that knows where its keys land in the file, so the routes and the page speak of
//! assets and the file can change under them. A person types prompts in this file, so each write finds its lines, changes only those,
//! keeps comments and `|`, and checks the file reads as the same project with that one change.
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use protocol::{
    error::{Error, Kind},
    yaml::to_json_keeping,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    folder::{PROJECT, prefix_of},
    format::{INSTRUCTIONS, LEADS, TEXT, canonical, invalid, names_a_file, yaml_key},
    graph::{is_name, mention_moved, name_key},
};

/// Where an asset's tool lands.
pub(super) const TOOL: &str = "tool";
const SECTION: &str = "assets";

fn by_hand(name: &str, why: impl std::fmt::Display) -> Error {
    invalid("project_edit", format!("{name} can't be written in {PROJECT}: {why}")).next(format!("edit {name} in {PROJECT} yourself"))
}

fn read(text: &str) -> Result<Value, Error> {
    to_json_keeping(text, names_a_file).map_err(|why| invalid("project_yaml", format!("{PROJECT}: {why}")))
}

fn pad(indent: usize) -> String {
    " ".repeat(indent)
}

/// The value `name` is under `assets:`.
fn slot<'a>(doc: &'a mut Value, name: &str) -> Option<&'a mut Value> {
    let key = doc[SECTION].as_object()?.keys().find(|key| name_key(key) == name_key(name)).cloned()?;
    doc[SECTION].get_mut(&key)
}

/// Where an asset's lines are in the text.
struct Place<'a> {
    lines: Vec<&'a str>,
    /// The line that names it.
    key: usize,
    /// The lines after it that are its body.
    body: Range<usize>,
    /// How far its fields are indented.
    child: usize,
    /// All on one line, `name: {…}`: what is before `{`, what is inside, what follows `}`.
    flow: Option<(String, String, String)>,
}

impl<'a> Place<'a> {
    /// The lines of `name` under `assets:`.
    fn find(text: &'a str, name: &str) -> Option<Self> {
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let start = lines.iter().position(|line| spaces(line) == 0 && split_comment(line).0 == "assets:")? + 1;
        let end = (start..lines.len()).find(|&n| significant(lines[n]) && spaces(lines[n]) == 0).unwrap_or(lines.len());
        let (key, body, child, flow) = block(&lines, start..end, name)?;
        Some(Self { lines, key, body, child, flow })
    }
}

/// Asset `name` on its own: the text before its lines, it alone under its section, and the text after. An edit reads and proves itself on this,
/// whatever else the project holds: the rest stays byte for byte, so what's true of it here is true of the file.
struct Window<'a> {
    before: &'a str,
    alone: String,
    after: &'a str,
}

fn window<'a>(text: &'a str, name: &str) -> Option<Window<'a>> {
    let place = Place::find(text, name)?;
    let end = end_of(&place.lines, place.key, place.body.end);
    let start: usize = place.lines[..place.key].iter().map(|line| line.len()).sum();
    let finish = start + place.lines[place.key..end].iter().map(|line| line.len()).sum::<usize>();
    Some(Window { before: &text[..start], alone: format!("{SECTION}:\n{}", &text[start..finish]), after: &text[finish..] })
}

/// `window`, for an edit: an anchor in these lines may be what another asset's alias reads, which only the whole file can prove.
fn editing<'a>(text: &'a str, name: &str) -> Option<Window<'a>> {
    window(text, name).filter(|at| !at.alone.contains('&'))
}

/// Asset `name`'s own lines as a project of one, for what needs only it read.
pub(super) fn alone(text: &str, name: &str) -> Option<String> {
    window(text, name).map(|at| format!("version: {}\n{}", super::folder::FORMAT, at.alone))
}

/// Asset `name`'s name as the file writes it, and its own lines under `assets:`, as `removed` answers them: what history keeps of it.
pub(super) fn own(text: &str, name: &str) -> Option<(String, String)> {
    let place = Place::find(text, name)?;
    let written = key_of(place.lines[place.key])?;
    Some((written, window(text, name)?.alone))
}

/// Whether `text` reads as YAML: a file a hand is halfway through saving may not.
pub(super) fn reads(text: &str) -> bool {
    read(text).is_ok()
}

/// What `own`'s lines say of the asset: its fields, or `None` when it's no mapping or doesn't read.
pub(super) fn fields_of(lines: &str) -> Option<Map<String, Value>> {
    let read = read(lines).ok()?;
    let (_, asset) = read[SECTION].as_object()?.iter().next()?;
    match asset {
        Value::Null => Some(Map::new()),
        asset => asset.as_object().cloned(),
    }
}

/// `text` with asset `name`'s own lines what `own` answered once, where they are. What they say is checked, and the rest stays byte for byte.
pub(super) fn replaced(text: &str, name: &str, back: &str) -> Result<String, Error> {
    let refused = || Error::new(Kind::Validation, "asset_text", format!("that isn't {name} as {PROJECT} held it"));
    let at = window(text, name).ok_or_else(|| by_hand(name, "it has no lines of its own"))?;
    let lines = back.strip_prefix(&format!("{SECTION}:\n")).ok_or_else(refused)?;
    let (from, to) = (lines.lines().next().map_or(0, spaces), at.alone.lines().nth(1).map_or(0, spaces));
    let block: String = lines.split_inclusive('\n').map(|line| if line.trim().is_empty() { line.to_owned() } else { format!("{}{}", pad(to), line.get(from.min(spaces(line))..).unwrap_or(line)) }).collect();
    let edited = [at.before, &block, at.after].concat();
    let wanted = read(back).map_err(|_| refused())?;
    let same = window(&edited, name).is_some_and(|now| read(&now.alone).is_ok_and(|read| canonical(&read) == canonical(&wanted)));
    let reads = read(&edited).is_ok();
    (same && reads).then_some(edited).ok_or_else(|| by_hand(name, "its lines as they were don't read where it is now"))
}

fn significant(line: &str) -> bool {
    !line.trim().is_empty() && !line.trim_start().starts_with('#')
}

fn spaces(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The lines of the key `name` among the mapping that fills `within`: the line opening it, its body, how far its fields are
/// indented, and for one all on one line, what is before `{`, what is inside, and what follows `}`.
#[allow(clippy::type_complexity)]
fn block(lines: &[&str], within: Range<usize>, name: &str) -> Option<(usize, Range<usize>, usize, Option<(String, String, String)>)> {
    let (start, end) = (within.start, within.end);
    let at = spaces(lines[start..end].iter().find(|line| significant(line))?);
    let key = (start..end).find(|&n| significant(lines[n]) && spaces(lines[n]) == at && key_of(lines[n]).is_some_and(|key| name_key(&key) == name_key(name)))?;
    let finish = ((key + 1)..end).find(|&n| significant(lines[n]) && spaces(lines[n]) <= at).unwrap_or(end);
    let first = ((key + 1)..finish).find(|&n| significant(lines[n]));
    let child = first.map_or(at + 2, |n| spaces(lines[n]));
    // Where its value starts: past the colon that ends the key.
    let after = lines[key].find(": ").map(|colon| colon + 2);
    let flow = after.and_then(|after| {
        let (head, value) = lines[key].split_at(after);
        let (written, _) = split_comment(value);
        let inner = written.strip_prefix('{')?.strip_suffix('}')?;
        let close = value.find(written)? + written.len();
        Some((head.to_owned(), inner.to_owned(), value[close..].to_owned()))
    });
    Some((key, (key + 1)..finish, child, flow))
}

/// The key a line opens: `name:`, `"name":`, `'name':`.
fn key_of(line: &str) -> Option<String> {
    let line = line.trim();
    if let Some(quote @ ('"' | '\'')) = line.chars().next() {
        let rest = &line[1..];
        let end = rest.find(quote)?;
        return rest[end + 1..].trim_start().starts_with(':').then(|| rest[..end].to_owned());
    }
    let colon = line.find(": ").or_else(|| line.strip_suffix(':').map(str::len))?;
    Some(line[..colon].to_owned())
}

/// A field's line as its indent and what follows `field:`.
fn field_rest<'a>(line: &'a str, field: &str) -> (&'a str, &'a str) {
    let spaces = line.len() - line.trim_start_matches(' ').len();
    (&line[..spaces], line[spaces + field.len()..].trim_start_matches(':').trim())
}

/// What a value writes, and its comment. A `#` in quotes is no comment, and neither is one with no space before it.
fn split_comment(rest: &str) -> (&str, Option<&str>) {
    let (mut quote, bytes) = (None, rest.as_bytes());
    for (n, byte) in bytes.iter().enumerate() {
        match (quote, *byte) {
            (Some(open), byte) if byte == open => quote = None,
            (None, b'"' | b'\'') => quote = Some(*byte),
            (None, b'#') if n == 0 || bytes[n - 1].is_ascii_whitespace() => return (rest[..n].trim(), Some(rest[n..].trim())),
            _ => {}
        }
    }
    (rest.trim(), None)
}

/// The ways to add `block`, a key's lines written from column 0, to `section`, keeping what is there, to try in turn. A mapping all on one
/// line takes only a single line, and a block of several needs lines of its own.
fn under_section(text: &str, section: &str, block: &str) -> Vec<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let indented = |indent: usize| block.split_inclusive('\n').map(|line| if line.trim().is_empty() { line.to_owned() } else { format!("{}{line}", " ".repeat(indent)) }).collect::<String>();
    let Some(at) = lines.iter().position(|line| spaces(line) == 0 && key_of(line).as_deref() == Some(section)) else {
        let glue = if text.is_empty() { "" } else if text.ends_with('\n') { "\n" } else { "\n\n" };
        return vec![format!("{text}{glue}{section}:\n{}", indented(2))];
    };
    let (written, comment) = split_comment(&lines[at][lines[at].find(':').expect("a key") + 1..]);
    let comment = comment.map_or(String::new(), |comment| format!("  {comment}"));
    match written {
        "" => {
            let end = ((at + 1)..lines.len()).find(|&n| significant(lines[n]) && spaces(lines[n]) == 0).unwrap_or(lines.len());
            let indent = ((at + 1)..end).find(|&n| significant(lines[n])).map_or(2, |n| spaces(lines[n]));
            let last = ((at + 1)..end).rev().find(|&n| significant(lines[n]) || (!lines[n].trim().is_empty() && spaces(lines[n]) > indent));
            // Past a text's last line, and what comments on it, unless that isn't the text's end, then at the section's end.
            [last.unwrap_or(at), end - 1].iter().map(|&after| insert_line(&lines, after, &indented(indent))).collect()
        }
        "{}" | "~" | "null" => vec![replace_line(&lines, at, &format!("{section}:{comment}\n{}", indented(2)))],
        written => match written.strip_prefix('{').and_then(|inner| inner.strip_suffix('}')) {
            Some(inner) if block.trim_end().lines().count() == 1 => vec![replace_line(&lines, at, &format!("{section}: {{{}, {}}}{comment}\n", block.trim_end(), inner.trim_start()))],
            _ => vec![],
        },
    }
}

fn replace_line(lines: &[&str], at: usize, line: &str) -> String {
    let mut kept: Vec<&str> = lines.to_vec();
    let ended = if line.ends_with('\n') || !lines[at].ends_with('\n') { line.to_owned() } else { format!("{line}\n") };
    kept[at] = &ended;
    kept.concat()
}

fn insert_line(lines: &[&str], after: usize, line: &str) -> String {
    let (head, tail) = lines.split_at(after + 1);
    let glue = if lines[after].ends_with('\n') { "" } else { "\n" };
    [head.concat().as_str(), glue, line, tail.concat().as_str()].concat()
}

/// Where an asset's own lines end: a blank line or a comment after them, at or left of its name, is the next one's.
fn end_of(lines: &[&str], key: usize, finish: usize) -> usize {
    let at = spaces(lines[key]);
    ((key + 1)..finish).rev().find(|&n| significant(lines[n]) || (!lines[n].trim().is_empty() && spaces(lines[n]) > at)).map_or(key + 1, |n| n + 1)
}

fn digest(lines: &[&str]) -> String {
    format!("\"{}\"", hex::encode(Sha256::digest(lines.concat())))
}

/// The digest of asset `name`'s own lines, so an edit of another moves nothing.
pub(super) fn etag(text: &str, name: &str) -> Option<String> {
    let place = Place::find(text, name)?;
    Some(digest(&place.lines[place.key..end_of(&place.lines, place.key, place.body.end)]))
}

/// Every asset's name key, read off its lines with no YAML: what a name has to keep clear of.
pub(super) fn names(text: &str) -> HashSet<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let Some(header) = lines.iter().position(|line| spaces(line) == 0 && key_of(line).as_deref() == Some(SECTION)) else { return HashSet::new() };
    let end = ((header + 1)..lines.len()).find(|&n| significant(lines[n]) && spaces(lines[n]) == 0).unwrap_or(lines.len());
    let Some(at) = lines[header + 1..end].iter().find(|line| significant(line)).map(|line| spaces(line)) else { return HashSet::new() };
    lines[header + 1..end].iter().filter(|line| significant(line) && spaces(line) == at).filter_map(|line| Some(name_key(&key_of(line)?))).collect()
}

/// Every asset's ETag, by name key, at once: a read of the project needn't look for each.
pub(super) fn etags(text: &str) -> HashMap<String, String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut found = HashMap::new();
    let Some(header) = lines.iter().position(|line| spaces(line) == 0 && key_of(line).as_deref() == Some(SECTION)) else { return found };
    let end = ((header + 1)..lines.len()).find(|&n| significant(lines[n]) && spaces(lines[n]) == 0).unwrap_or(lines.len());
    let Some(at) = lines[header + 1..end].iter().find(|line| significant(line)).map(|line| spaces(line)) else { return found };
    let keys: Vec<usize> = ((header + 1)..end).filter(|&n| significant(lines[n]) && spaces(lines[n]) == at).collect();
    for (n, &key) in keys.iter().enumerate() {
        let finish = keys.get(n + 1).copied().unwrap_or(end);
        if let Some(name) = key_of(lines[key]) {
            found.insert(name_key(&name), digest(&lines[key..end_of(&lines, key, finish)]));
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

/// A list's item at `indent`: a mapping's first field on the dash's line, a beat's `duration` first.
fn item_lines(item: &Value, indent: usize) -> Vec<String> {
    match item {
        Value::Object(fields) if !fields.is_empty() => {
            let (first, rest): (Vec<_>, Vec<_>) = fields.iter().partition(|(field, _)| field.as_str() == "duration");
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

/// `field` set in what the file reads as.
fn expect(wanted: &mut Value, name: &str, field: &str, value: &Value) -> Result<(), String> {
    let asset = slot(wanted, name).ok_or("it isn't in the file")?;
    if asset.is_null() {
        *asset = json!({});
    }
    let asset = asset.as_object_mut().ok_or("it is no mapping of keys")?;
    match value {
        Value::Null => drop(asset.remove(field)),
        value => drop(asset.insert(field.to_owned(), value.clone())),
    }
    Ok(())
}

/// What the file says of a project, with what an empty section or asset reads as made the same.
fn tidied(mut value: Value, name: &str) -> Value {
    if let Some(asset) = slot(&mut value, name).filter(|asset| asset.is_null()) {
        *asset = json!({});
    }
    if value.get(SECTION).is_some_and(|held| held.is_null() || held.as_object().is_some_and(Map::is_empty)) {
        value.as_object_mut().expect("a section's mapping").remove(SECTION);
    }
    canonical(&value)
}

/// `text` with one key of asset `name` written: its lines replaced, or new ones after what's there. Whether a comment inside them went.
fn write_field(text: &str, name: &str, field: &str, value: &Value) -> Result<(String, bool), Error> {
    let place = Place::find(text, name).ok_or_else(|| by_hand(name, "it has no lines of its own"))?;
    let mut lines: Vec<String> = place.lines.iter().map(|line| if line.ends_with('\n') { (*line).to_owned() } else { format!("{line}\n") }).collect();
    let (key, mut indent) = (place.key, place.child);
    let mut within = (key + 1)..end_of(&place.lines, key, place.body.end);
    if let Some((head, inner, tail)) = &place.flow {
        let fields = flow_lines(inner, spaces(&lines[key]) + 2);
        indent = spaces(&lines[key]) + 2;
        lines[key] = format!("{}{}\n", head.trim_end(), tail.trim_end());
        let count = fields.len();
        lines.splice((key + 1)..(key + 1), fields);
        within = (key + 1)..(key + 1 + count);
    }
    let held = span_of(&lines, &within, indent, field);
    let lost = held.as_ref().is_some_and(|span| !value.is_null() && holds_comment(&lines[span.clone()]));
    match (held, value) {
        (None, Value::Null) => {}
        (Some(span), Value::Null) => drop(lines.drain(span)),
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

/// `text` with these keys of asset `name` set, a null cleared, and what went doing so. Setting what it holds changes nothing.
pub(super) fn set(text: &str, name: &str, fields: &Map<String, Value>) -> Result<(String, Vec<String>), Error> {
    if let Some(at) = editing(text, name)
        && let Ok((edited, warnings)) = set_in(&at.alone, name, fields)
        && let Some(lines) = edited.strip_prefix(&format!("{SECTION}:\n"))
    {
        return Ok(([at.before, lines, at.after].concat(), warnings));
    }
    set_in(text, name, fields)
}

fn set_in(text: &str, name: &str, fields: &Map<String, Value>) -> Result<(String, Vec<String>), Error> {
    let mut wanted = read(text)?;
    let (mut edited, mut warnings) = (text.to_owned(), vec![]);
    // An Edit's clips, as the file holds them.
    let (translated, said) = super::tools::in_the_file(fields)?;
    warnings.extend(said);
    for (field, value) in &translated {
        let asset = slot(&mut wanted, name).ok_or_else(|| by_hand(name, "it has no lines of its own"))?;
        if asset.get(field).map_or(value.is_null(), |held| canonical(held) == canonical(value)) {
            continue;
        }
        expect(&mut wanted, name, field, value).map_err(|why| by_hand(name, why))?;
        let (next, lost) = write_field(&edited, name, field, value)?;
        edited = next;
        if lost {
            warnings.push(format!("a comment inside {name}'s {field} was lost"));
        }
    }
    (tidied(read(&edited)?, name) == tidied(wanted, name)).then_some(()).ok_or_else(|| by_hand(name, "the edit would change something else"))?;
    Ok((edited, warnings))
}

/// What a change of its tool writes of asset `name`, with what `fields` ask, `tool` empty taking it off. Nothing else moves: its pictures, numbers, approval and
/// description stay, and its text. Only a box with none of those pictures has its words in another field: `text` with no tool, `instructions` with one. A new
/// prompt on a picture, `holds`, starts by reading it.
pub(super) fn with_tool(text: &str, name: &str, tool: &str, holds: Option<u32>, fields: &mut Map<String, Value>) -> Result<(), Error> {
    let mut read = read(&editing(text, name).map_or_else(|| text.to_owned(), |at| at.alone))?;
    let held = slot(&mut read, name).ok_or_else(|| by_hand(name, "it has no lines of its own"))?.as_object().cloned().unwrap_or_default();
    let said = fields.contains_key(INSTRUCTIONS) || fields.contains_key(TEXT);
    let moved = |fields: &mut Map<String, Value>, from: &str, to: &str| {
        if let Some(words) = held.get(from).filter(|_| !said) {
            fields.insert(to.to_owned(), words.clone());
            fields.insert(from.to_owned(), Value::Null);
        }
    };
    match (tool.is_empty(), holds) {
        (true, None) => LEADS.iter().rev().find(|field| held.contains_key(**field)).into_iter().for_each(|field| moved(fields, field, TEXT)),
        (false, None) => moved(fields, TEXT, INSTRUCTIONS),
        (false, Some(number)) if !said && !held.contains_key(INSTRUCTIONS) && !held.contains_key(TOOL) => drop(fields.insert(INSTRUCTIONS.to_owned(), json!(format!("<self.{number}>")))),
        _ => {}
    }
    fields.insert(TOOL.to_owned(), if tool.is_empty() { Value::Null } else { json!(tool) });
    Ok(())
}

/// `text` with a new asset at the end of `assets:`, made by `tool`, or with none a text asset.
pub(super) fn added(text: &str, name: &str, tool: &str, fields: &Map<String, Value>) -> Result<(String, Vec<String>), Error> {
    let block = match tool.is_empty() {
        true => format!("{}:\n  {}: \"\"\n", yaml_key(name), super::format::TEXT),
        false => format!("{}:\n  tool: {}\n", yaml_key(name), inline(&json!(tool))),
    };
    let edited = add_block(text, name, &block)?;
    set(&edited, name, fields)
}

/// `text` with one file more, on a line of its own: a name and the file it holds.
pub(super) fn constant(text: &str, name: &str, sha256: &str) -> Result<String, Error> {
    add_block(text, name, &format!("{}: {{approved: {}}}\n", yaml_key(name), inline(&json!(prefix_of(sha256)))))
}

/// `text` with asset `name` holding that file.
pub(super) fn approved(text: &str, name: &str, sha256: &str) -> Result<String, Error> {
    set(text, name, &Map::from_iter([("approved".to_owned(), json!(prefix_of(sha256)))])).map(|(edited, _)| edited)
}

/// `text` with asset `name` holding no file: its state follows its inputs again.
pub(super) fn unapproved(text: &str, name: &str) -> Result<String, Error> {
    set(text, name, &Map::from_iter([("approved".to_owned(), Value::Null)])).map(|(edited, _)| edited)
}

/// `text` with asset `from` called `to`: its key, and every `<from>` and `<from.2>`. Comments and formatting stay.
pub(super) fn renamed(text: &str, from: &str, to: &str) -> Result<String, Error> {
    let not_its_own = || by_hand(from, "it has no line of its own under `assets:`");
    // `<from.2>` is a version of it, unless something from before names couldn't end so goes by that.
    let held = names(text);
    let said = text_renamed(text, &|name: &str| mention_moved(name, from, to).filter(|_| name_key(name) == name_key(from) || !held.contains(&name_key(name))));
    let place = Place::find(&said, from).ok_or_else(not_its_own)?;
    let written = key_of(place.lines[place.key]).ok_or_else(not_its_own)?;
    Ok(replace_line(&place.lines, place.key, &place.lines[place.key].replacen(&written, &yaml_key(to), 1)))
}

fn add_block(text: &str, name: &str, block: &str) -> Result<String, Error> {
    // In its section, where a hash reads as a hash.
    let framed: String = block.split_inclusive('\n').map(|line| format!("  {line}")).collect();
    let framed = read(&format!("{SECTION}:\n{framed}")).map_err(|_| by_hand(name, "its lines don't read"))?;
    // All that was there, byte for byte, around lines that read as this asset: the rest needn't be read to know.
    let alone = canonical(&framed);
    let added = |edited: &String| window(edited, name).is_some_and(|at| [at.before, at.after].concat() == text && read(&at.alone).is_ok_and(|read| canonical(&read) == alone));
    if let Some(edited) = under_section(text, SECTION, block).into_iter().find(added) {
        return Ok(edited);
    }
    let mut wanted = read(text)?;
    if wanted.is_null() {
        wanted = json!({});
    }
    let top = wanted.as_object_mut().ok_or_else(|| by_hand(name, "the file is no mapping of sections"))?;
    let held = top.entry(SECTION).or_insert(Value::Null);
    if held.is_null() {
        *held = json!({});
    }
    let body = framed[SECTION].as_object().and_then(|block| block.values().next()).cloned().unwrap_or(Value::Null);
    held.as_object_mut().ok_or_else(|| by_hand(name, format!("its `{SECTION}:` is no mapping")))?.insert(name.to_owned(), body);
    let wanted = canonical(&wanted);
    let same = |edited: &String| read(edited).is_ok_and(|read| canonical(&read) == wanted);
    under_section(text, SECTION, block).into_iter().find(same).ok_or_else(|| by_hand(name, "the edit would change something else"))
}

/// `text` with asset `name`'s lines gone, and what puts them back.
pub(super) fn removed(text: &str, name: &str) -> Result<(String, String), Error> {
    if let Some(at) = editing(text, name)
        && let Ok((_, back)) = removed_from(&at.alone, name)
    {
        return Ok(([at.before, at.after].concat(), back));
    }
    removed_from(text, name)
}

fn removed_from(text: &str, name: &str) -> Result<(String, String), Error> {
    let place = Place::find(text, name).ok_or_else(|| by_hand(name, "it has no lines of its own"))?;
    let end = end_of(&place.lines, place.key, place.body.end);
    let back = format!("{SECTION}:\n{}", place.lines[place.key..end].concat());
    let edited = [place.lines[..place.key].concat(), place.lines[end..].concat()].concat();
    let mut wanted = read(text)?;
    let key = wanted[SECTION].as_object().and_then(|held| held.keys().find(|key| name_key(key) == name_key(name)).cloned());
    let key = key.ok_or_else(|| by_hand(name, "it is part of what's around it"))?;
    wanted[SECTION].as_object_mut().expect("a section's mapping").remove(&key);
    (tidied(read(&edited)?, name) == tidied(wanted, name)).then_some(()).ok_or_else(|| by_hand(name, "the edit would change something else"))?;
    Ok((edited, back))
}

/// `text` with what `removed` said back in `assets:`, under the name it had.
pub(super) fn restored(text: &str, name: &str, back: &str) -> Result<String, Error> {
    let refused = || Error::new(Kind::Validation, "asset_text", format!("that isn't what deleting {name} answered"));
    let (header, lines) = back.split_once('\n').ok_or_else(refused)?;
    if header.strip_suffix(':') != Some(SECTION) {
        return Err(refused());
    }
    let at = lines.lines().next().map_or(0, spaces);
    if lines.lines().next().and_then(key_of).is_none_or(|key| name_key(&key) != name_key(name)) || read(back).is_err() {
        return Err(refused());
    }
    let block: String = lines.split_inclusive('\n').map(|line| line.get(at..).filter(|_| spaces(line) >= at).unwrap_or(line)).collect();
    add_block(text, name, &block)
}
/// `value` with each `<name>` in it as `renamed` says.
pub(super) fn mentions_renamed(value: &mut Value, renamed: &dyn Fn(&str) -> Option<String>) {
    match value {
        Value::String(text) => *text = text_renamed(text, renamed),
        Value::Array(items) => items.iter_mut().for_each(|item| mentions_renamed(item, renamed)),
        Value::Object(fields) => fields.values_mut().for_each(|field| mentions_renamed(field, renamed)),
        _ => {}
    }
}

fn text_renamed(text: &str, renamed: &dyn Fn(&str) -> Option<String>) -> String {
    let (mut said, mut rest) = (String::with_capacity(text.len()), text);
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let inner = after.find(['<', '>', '\n']).filter(|&end| after.as_bytes()[end] == b'>').map(|end| &after[..end]);
        said.push_str(&rest[..=open]);
        match inner.filter(|inner| is_name(inner)).and_then(|inner| Some((inner, renamed(inner)?))) {
            Some((inner, to)) => {
                said.push_str(&to);
                rest = &after[inner.len()..];
            }
            None => rest = after,
        }
    }
    said.push_str(rest);
    said
}

