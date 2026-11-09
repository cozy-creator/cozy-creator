//! YAML as JSON: what a person writes (`cozy run --input`, a project's project.yaml) read once into
//! the JSON every typed reader takes.
use saphyr::{Scalar, Yaml, YamlLoader};
use saphyr_parser::Parser;
use serde_json::{Number, Value};

/// One YAML document's nodes, each scalar as it is written, read only once a reader says what it needs.
fn documents(text: &str) -> Result<Vec<Yaml<'_>>, String> {
    let mut loader = YamlLoader::<Yaml>::default();
    loader.early_parse(false);
    Parser::new_from_str(text).load(&mut loader, true).map_err(|e| e.to_string())?;
    match loader.error() {
        Some(error) => Err(error.to_string()),
        None => Ok(loader.into_documents()),
    }
}

/// A scalar as it is written, what a mapping's key is.
fn written<'n>(node: &'n Yaml<'_>) -> Option<&'n str> {
    match node {
        Yaml::Representation(text, ..) => Some(text),
        Yaml::Tagged(_, inner) => written(inner),
        _ => None,
    }
}

/// One YAML document (1.2 core schema, aliases resolved) as JSON; a mapping's keys are its scalar keys' text. A number JSON can't
/// hold is the text it was written as.
pub fn to_json(text: &str) -> Result<Value, String> {
    to_json_keeping(text, |_| false)
}

/// `to_json`, where a number `as_written` says of its path, the mapping keys above it, is its text: `007`, `12345678e012` and
/// `80644e452548` are three hashes' prefixes there, and three numbers anywhere else.
pub fn to_json_keeping(text: &str, as_written: impl Fn(&[&str]) -> bool) -> Result<Value, String> {
    Ok(to_json_and_keys(text, &[], as_written)?.0)
}

/// `to_json_keeping`, and what `keys` says of `path`, of the one read of `text` that both come from.
pub fn to_json_and_keys(text: &str, path: &[&str], as_written: impl Fn(&[&str]) -> bool) -> Result<(Value, Vec<String>), String> {
    fn json<'n>(node: &'n Yaml<'_>, path: &mut Vec<&'n str>, as_written: &dyn Fn(&[&str]) -> bool) -> Result<Value, String> {
        Ok(match node {
            Yaml::Representation(text, style, tag) => {
                let scalar = Scalar::parse_from_cow_and_metadata(text.clone(), *style, tag.as_ref()).ok_or_else(|| format!("{text}: its tag says it is something else"))?;
                match scalar {
                    Scalar::Null => Value::Null,
                    Scalar::Boolean(b) => Value::Bool(b),
                    Scalar::Integer(_) | Scalar::FloatingPoint(_) if as_written(path) => Value::String(text.to_string()),
                    Scalar::Integer(i) => Value::from(i),
                    Scalar::FloatingPoint(f) => Number::from_f64(f.into_inner()).map_or_else(|| Value::String(text.to_string()), Value::Number),
                    Scalar::String(s) => Value::String(s.to_string()),
                }
            }
            Yaml::Sequence(items) => Value::Array(items.iter().map(|item| json(item, path, as_written)).collect::<Result<_, _>>()?),
            Yaml::Mapping(map) => {
                let mut fields = serde_json::Map::new();
                for (key, value) in map {
                    let key = written(key).ok_or("a mapping key that is not a scalar")?;
                    path.push(key);
                    fields.insert(key.to_owned(), json(value, path, as_written)?);
                    path.pop();
                }
                Value::Object(fields)
            }
            Yaml::Tagged(_, inner) => json(inner, path, as_written)?,
            Yaml::Value(_) | Yaml::Alias(_) | Yaml::BadValue => return Err("an alias to no anchor".into()),
        })
    }
    match documents(text)?.as_slice() {
        [document] => Ok((json(document, &mut vec![], &as_written)?, keys_of(document, path))),
        [] => Ok((Value::Null, vec![])),
        _ => Err("more than one YAML document".into()),
    }
}

/// The keys of the mapping at `path` in one YAML document, as written, in the order the text writes them. JSON's maps have no order.
pub fn keys(text: &str, path: &[&str]) -> Vec<String> {
    keys_at(text, path).unwrap_or_default()
}

/// The keys of the mapping at `path`, none for one left bare; no answer if the text doesn't read or has nothing there, which a half-made save may not.
pub fn keys_at(text: &str, path: &[&str]) -> Option<Vec<String>> {
    keys_in(documents(text).ok()?.first()?, path)
}

/// `keys_at` of each key of the mapping at `path`, from one read of `text`: each child's own keys, none where it's no mapping. No answer
/// as `keys_at` gives none.
pub fn child_keys_at(text: &str, path: &[&str]) -> Option<Vec<(String, Vec<String>)>> {
    fn keys(node: &Yaml<'_>) -> Vec<String> {
        match node {
            Yaml::Mapping(map) => map.iter().filter_map(|(key, _)| written(key).map(str::to_owned)).collect(),
            _ => vec![],
        }
    }
    let documents = documents(text).ok()?;
    let mut at = documents.first()?;
    for part in path {
        let Yaml::Mapping(map) = at else { return None };
        at = map.iter().find(|(key, _)| written(key) == Some(*part)).map(|(_, value)| value)?;
    }
    match at {
        Yaml::Mapping(map) => Some(map.iter().filter_map(|(key, value)| Some((written(key)?.to_owned(), keys(value)))).collect()),
        Yaml::Representation(text, ..) if matches!(text.as_ref(), "" | "~" | "null") => Some(vec![]),
        _ => None,
    }
}

fn keys_of(document: &Yaml<'_>, path: &[&str]) -> Vec<String> {
    keys_in(document, path).unwrap_or_default()
}

fn keys_in(document: &Yaml<'_>, path: &[&str]) -> Option<Vec<String>> {
    let mut at = document;
    for part in path {
        let Yaml::Mapping(map) = at else { return None };
        at = map.iter().find(|(key, _)| written(key) == Some(*part)).map(|(_, value)| value)?;
    }
    match at {
        Yaml::Mapping(map) => Some(map.iter().filter_map(|(key, _)| written(key).map(str::to_owned)).collect()),
        Yaml::Representation(text, ..) if matches!(text.as_ref(), "" | "~" | "null") => Some(vec![]),
        _ => None,
    }
}

/// `value` as YAML a diff can follow: each key on its own line, in the order it holds them, a string plain where that reads back as that
/// string and otherwise quoted as JSON, which is YAML.
pub fn from_json(value: &Value) -> String {
    let mut out = String::new();
    write_node(value, 0, &mut out);
    out
}

fn write_node(value: &Value, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match value {
        Value::Object(fields) if !fields.is_empty() => {
            for (key, item) in fields {
                out.push_str(&pad);
                out.push_str(&write_text(key));
                out.push(':');
                match item {
                    Value::Object(inner) if !inner.is_empty() => {
                        out.push('\n');
                        write_node(item, indent + 2, out);
                    }
                    Value::Array(inner) if !inner.is_empty() => {
                        out.push('\n');
                        write_node(item, indent + 2, out);
                    }
                    _ => {
                        out.push(' ');
                        write_node(item, 0, out);
                    }
                }
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for item in items {
                let mut inner = String::new();
                write_node(item, indent + 2, &mut inner);
                out.push_str(&pad);
                out.push_str("- ");
                out.push_str(inner.trim_start_matches(' '));
            }
        }
        scalar => {
            out.push_str(&pad);
            match scalar {
                Value::String(text) => out.push_str(&write_text(text)),
                Value::Object(_) => out.push_str("{}"),
                Value::Array(_) => out.push_str("[]"),
                other => out.push_str(&other.to_string()),
            }
            out.push('\n');
        }
    }
}

/// `text` plain when nothing could read it as anything else: a hash, a name, a path. Anything starting with a digit may be a number.
fn write_text(text: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_/.:+@=-".contains(c);
    let starts = text.chars().next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == '/');
    let word = matches!(text.to_ascii_lowercase().as_str(), "null" | "true" | "false");
    match starts && text.chars().all(safe) && !text.ends_with(':') && !word {
        true => text.to_owned(),
        false => Value::from(text).to_string(),
    }
}
