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
    let Ok(documents) = documents(text) else { return vec![] };
    documents.first().map_or_else(Vec::new, |document| keys_of(document, path))
}

fn keys_of(document: &Yaml<'_>, path: &[&str]) -> Vec<String> {
    let mut at = document;
    for part in path {
        let Yaml::Mapping(map) = at else { return vec![] };
        let Some(next) = map.iter().find(|(key, _)| written(key) == Some(*part)).map(|(_, value)| value) else { return vec![] };
        at = next;
    }
    match at {
        Yaml::Mapping(map) => map.iter().filter_map(|(key, _)| written(key).map(str::to_owned)).collect(),
        _ => vec![],
    }
}
