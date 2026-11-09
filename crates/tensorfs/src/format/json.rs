//! Strict reads and deterministic writes, for the manifest and the repo (JSON documents).
//!
//! Read: one JSON object with no duplicate keys; the typed layer then takes what it reads and ignores
//! unknown fields, whatever they hold (a newer writer's null or float). Write: sorted keys (byte order), no whitespace, integers
//! only, serde_json's string escaping (the same as Python's `json.dumps(ensure_ascii=False)`).

use std::collections::HashSet;
use std::fmt;

use serde::de::{DeserializeOwned, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Serialize;
use serde_json::Value;

use super::{check_format, invalid, Config, ConfigValue, Error, Result};

/// Parse a document whose `format` tag must be `expected` (any minor).
pub(crate) fn read<T: DeserializeOwned>(bytes: &[u8], expected: &'static str) -> Result<T> {
    let bad = |e: serde_json::Error| Error::Invalid(e.to_string());
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let tag = de.deserialize_map(Root).map_err(bad)?;
    de.end().map_err(bad)?;
    match tag {
        Some(found) => check_format(&found, expected)?,
        None => return invalid(format!("no `format` tag (want {expected})")),
    }
    serde_json::from_slice(bytes).map_err(bad)
}

pub(crate) fn write<T: Serialize>(doc: &T) -> Result<Vec<u8>> {
    let value = serde_json::to_value(doc).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut out = Vec::with_capacity(4096);
    emit(&value, &mut out)?;
    Ok(out)
}

fn emit(v: &Value, out: &mut Vec<u8>) -> Result<()> {
    match v {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Number(n) if n.is_f64() => return invalid(format!("{n} is not an integer")),
        Value::Number(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Value::String(s) => serde_json::to_writer(&mut *out, s).expect("a string serializes"),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                emit(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            out.push(b'{');
            for (i, (k, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                serde_json::to_writer(&mut *out, k).expect("a key serializes");
                out.push(b':');
                emit(item, out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

/// The root object: checked like [`Strict`], and its `format` string returned.
struct Root;

impl<'de> Visitor<'de> for Root {
    type Value = Option<String>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON object")
    }
    fn visit_map<A: MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let (mut seen, mut tag) = (HashSet::new(), None);
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!("duplicate key {key:?}")));
            }
            if key == "format" {
                tag = map.next_value::<Value>()?.as_str().map(str::to_owned);
            } else {
                map.next_value_seed(Strict)?;
            }
        }
        Ok(tag)
    }
}

/// Walks a value refusing duplicate keys, anywhere.
struct Strict;

impl<'de> DeserializeSeed<'de> for Strict {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Strict {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E>(self, _: bool) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_i64<E>(self, _: i64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_u64<E>(self, _: u64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_f64<E>(self, _: f64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_str<E>(self, _: &str) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_unit<E>(self) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<(), A::Error> {
        while seq.next_element_seed(Strict)?.is_some() {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!("duplicate key {key:?}")));
            }
            map.next_value_seed(Strict)?;
        }
        Ok(())
    }
}

/// A config as the Diffusers and Transformers writers write one, `json.dumps(config, indent=2,
/// sort_keys=True) + "\n"`, with Python's float spelling (`1e-05`, `1.0`).
pub fn config_json(config: &Config) -> Vec<u8> {
    let mut out = String::new();
    map(&mut out, config, 0);
    out.push('\n');
    out.into_bytes()
}

fn map(out: &mut String, m: &Config, depth: usize) {
    let mut sorted: Vec<_> = m.iter().collect();
    sorted.sort_by_key(|(k, _)| *k);
    nested(out, '{', '}', depth, sorted.into_iter(), |out, (k, v)| {
        out.push_str(&serde_json::to_string(k).expect("a string serializes"));
        out.push_str(": ");
        value(out, v, depth + 1);
    });
}

fn nested<T>(
    out: &mut String,
    open: char,
    close: char,
    depth: usize,
    items: impl ExactSizeIterator<Item = T>,
    mut each: impl FnMut(&mut String, T),
) {
    out.push(open);
    let empty = items.len() == 0;
    for (i, item) in items.enumerate() {
        out.push_str(if i == 0 { "\n" } else { ",\n" });
        out.push_str(&"  ".repeat(depth + 1));
        each(out, item);
    }
    if !empty {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    }
    out.push(close);
}

fn value(out: &mut String, v: &ConfigValue, depth: usize) {
    match v {
        ConfigValue::Number(n) if n.is_f64() => {
            out.push_str(&python_float(n.as_f64().expect("a float")))
        }
        ConfigValue::Array(l) => nested(out, '[', ']', depth, l.iter(), |out, v| {
            value(out, v, depth + 1)
        }),
        ConfigValue::Object(m) => map(out, m, depth),
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// Python's `repr` of a finite float: the shortest round-trip digits (ryu's, the closest of them),
/// positional for decimal exponents -4 through 15, else `d.ddde±XX`.
fn python_float(f: f64) -> String {
    let ryu = serde_json::to_string(&f).expect("a finite float");
    let (sign, ryu) = ryu
        .strip_prefix('-')
        .map_or(("", ryu.as_str()), |r| ("-", r));
    let (mantissa, exp) = ryu.split_once('e').unwrap_or((ryu, "0"));
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let all = format!("{int}{frac}");
    let digits = all.trim_start_matches('0').trim_end_matches('0');
    if digits.is_empty() {
        return format!("{sign}0.0");
    }
    let lead = (all.len() - all.trim_start_matches('0').len()) as i32;
    let exp = exp.parse::<i32>().expect("an integer exponent") + int.len() as i32 - 1 - lead;
    if (-4..16).contains(&exp) {
        let point = exp + 1;
        if point <= 0 {
            return format!("{sign}0.{}{digits}", "0".repeat(-point as usize));
        }
        let point = point as usize;
        return match digits.len() <= point {
            true => format!("{sign}{digits}{}.0", "0".repeat(point - digits.len())),
            false => format!("{sign}{}.{}", &digits[..point], &digits[point..]),
        };
    }
    let mantissa = match digits.len() {
        1 => digits.to_owned(),
        _ => format!("{}.{}", &digits[..1], &digits[1..]),
    };
    format!(
        "{sign}{mantissa}e{}{:02}",
        if exp < 0 { '-' } else { '+' },
        exp.abs()
    )
}
