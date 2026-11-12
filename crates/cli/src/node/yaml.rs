//! A file of nodes as YAML: block style, keys in the order given, a string plain where every YAML reader (1.1 and 1.2) reads it back as
//! the same string, else quoted, and words of several lines as a literal block.
use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// `nodes` as a file, each node's count of takes left out as the comment under its `takes:`.
pub(crate) fn file(nodes: &Map<String, Value>, older: &BTreeMap<String, usize>) -> String {
    let mut out = String::new();
    for (name, node) in nodes {
        out.push_str(&key(name));
        out.push(':');
        match node {
            Value::Object(fields) if !fields.is_empty() => {
                out.push('\n');
                for (field, value) in fields {
                    out.push_str("  ");
                    out.push_str(&key(field));
                    out.push(':');
                    match (field.as_str(), older.get(name).filter(|count| **count > 0)) {
                        ("takes", Some(count)) => {
                            out.push_str(&format!("\n    # {count} older take{}: cozy get {name} --takes", if *count == 1 { "" } else { "s" }));
                            match value {
                                Value::Array(items) if !items.is_empty() => sequence(&mut out, items, 4),
                                _ => out.push('\n'),
                            }
                        }
                        _ => after_key(&mut out, value, 2),
                    }
                }
            }
            other => after_key(&mut out, other, 0),
        }
    }
    out
}

/// `value` after a `key:` written at column `col`.
fn after_key(out: &mut String, value: &Value, col: usize) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            out.push('\n');
            mapping(out, map, col + 2, false);
        }
        Value::Array(items) if !items.is_empty() => sequence(out, items, col + 2),
        Value::String(text) if text.contains('\n') && literal(text) => {
            out.push_str(&block_header(text));
            out.push('\n');
            block_lines(out, text, col + 2);
        }
        scalar => {
            out.push(' ');
            out.push_str(&inline(scalar));
            out.push('\n');
        }
    }
}

/// Each key of `map` at column `col`; the first already placed when `placed` (after a `- `).
fn mapping(out: &mut String, map: &Map<String, Value>, col: usize, placed: bool) {
    for (at, (field, value)) in map.iter().enumerate() {
        if at > 0 || !placed {
            out.push_str(&" ".repeat(col));
        }
        out.push_str(&key(field));
        out.push(':');
        after_key(out, value, col);
    }
}

/// `items` as a block sequence, each `- ` at column `col`, starting on a new line.
fn sequence(out: &mut String, items: &[Value], col: usize) {
    out.push('\n');
    for item in items {
        out.push_str(&" ".repeat(col));
        out.push('-');
        match item {
            Value::Object(map) if !map.is_empty() => {
                out.push(' ');
                mapping(out, map, col + 2, true);
            }
            Value::Array(inner) if !inner.is_empty() => sequence(out, inner, col + 2),
            Value::String(text) if text.contains('\n') && literal(text) => {
                out.push(' ');
                out.push_str(&block_header(text));
                out.push('\n');
                block_lines(out, text, col + 2);
            }
            scalar => {
                out.push(' ');
                out.push_str(&inline(scalar));
                out.push('\n');
            }
        }
    }
}

/// A one-line value.
fn inline(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => match plain(text) {
            true => text.clone(),
            false => quoted(text),
        },
        Value::Array(_) => "[]".to_owned(),
        Value::Object(_) => "{}".to_owned(),
    }
}

fn key(text: &str) -> String {
    if plain(text) { text.to_owned() } else { quoted(text) }
}

/// YAML's double quotes take JSON's escapes.
fn quoted(text: &str) -> String {
    serde_json::to_string(text).expect("a string serializes")
}

/// Whether `text` reads back as itself unquoted, in YAML 1.2 and in the 1.1 readers agents use (PyYAML: `yes`, `9:16` as a number).
fn plain(text: &str) -> bool {
    const WORDS: [&str; 10] = ["null", "~", "true", "false", "yes", "no", "on", "off", "y", "n"];
    let Some(first) = text.chars().next() else { return false };
    let second = text.chars().nth(1);
    let numeric = first.is_ascii_digit() || (matches!(first, '+' | '-' | '.') && second.is_some_and(|c| c.is_ascii_digit() || c == '.'));
    !numeric
        && !WORDS.contains(&text.to_lowercase().as_str())
        && !text.to_lowercase().starts_with(".inf")
        && !text.to_lowercase().starts_with(".nan")
        && !"-?:,[]{}#&*!|>'\"%@`".contains(first)
        && !first.is_whitespace()
        && !text.ends_with(char::is_whitespace)
        && !text.ends_with(':')
        && !text.contains(": ")
        && !text.contains(" #")
        && !text.chars().any(|c| c.is_control())
}

/// Whether `text` can be a literal block: some words, and no control character but a newline or a tab.
fn literal(text: &str) -> bool {
    text.chars().any(|c| !c.is_whitespace()) && !text.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
}

/// `|`, its indentation said when its first line starts with a space, and how its last newlines go.
fn block_header(text: &str) -> String {
    let trailing = text.len() - text.trim_end_matches('\n').len();
    let chomp = match trailing {
        0 => "-",
        1 => "",
        _ => "+",
    };
    let indent = if text.starts_with([' ', '\t']) { "2" } else { "" };
    format!(" |{indent}{chomp}")
}

fn block_lines(out: &mut String, text: &str, col: usize) {
    let body = text.strip_suffix('\n').unwrap_or(text);
    for line in body.split('\n') {
        if !line.is_empty() {
            out.push_str(&" ".repeat(col));
            out.push_str(line);
        }
        out.push('\n');
    }
}
