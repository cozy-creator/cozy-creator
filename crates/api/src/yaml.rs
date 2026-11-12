//! YAML as JSON: a run's input as a person writes it (`cozy run --input`), read once into the JSON every
//! typed reader takes.
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
    fn json(node: &Yaml<'_>) -> Result<Value, String> {
        Ok(match node {
            Yaml::Representation(text, style, tag) => {
                let scalar = Scalar::parse_from_cow_and_metadata(text.clone(), *style, tag.as_ref()).ok_or_else(|| format!("{text}: its tag says it is something else"))?;
                match scalar {
                    Scalar::Null => Value::Null,
                    Scalar::Boolean(b) => Value::Bool(b),
                    Scalar::Integer(i) => Value::from(i),
                    Scalar::FloatingPoint(f) => Number::from_f64(f.into_inner()).map_or_else(|| Value::String(text.to_string()), Value::Number),
                    Scalar::String(s) => Value::String(s.to_string()),
                }
            }
            Yaml::Sequence(items) => Value::Array(items.iter().map(json).collect::<Result<_, _>>()?),
            Yaml::Mapping(map) => {
                let mut fields = serde_json::Map::new();
                for (key, value) in map {
                    let key = written(key).ok_or("a mapping key that is not a scalar")?;
                    fields.insert(key.to_owned(), json(value)?);
                }
                Value::Object(fields)
            }
            Yaml::Tagged(_, inner) => json(inner)?,
            Yaml::Value(_) | Yaml::Alias(_) | Yaml::BadValue => return Err("an alias to no anchor".into()),
        })
    }
    match documents(text)?.as_slice() {
        [document] => json(document),
        [] => Ok(Value::Null),
        _ => Err("more than one YAML document".into()),
    }
}
