//! YAML as JSON: what a person writes (`cozy run --input`, a project's project.yaml) read once into
//! the JSON every typed reader takes.
use saphyr::{LoadableYamlNode, Scalar, Yaml};
use serde_json::{Number, Value};

/// One YAML document (1.2 core schema, aliases resolved) as JSON; a mapping's keys are its
/// scalar keys' text.
pub fn to_json(text: &str) -> Result<Value, String> {
    fn json(node: &Yaml) -> Result<Value, String> {
        Ok(match node {
            Yaml::Value(Scalar::Null) => Value::Null,
            Yaml::Value(Scalar::Boolean(b)) => Value::Bool(*b),
            Yaml::Value(Scalar::Integer(i)) => Value::from(*i),
            Yaml::Value(Scalar::FloatingPoint(f)) => {
                Value::Number(Number::from_f64(f.into_inner()).ok_or("a number JSON cannot hold")?)
            }
            Yaml::Value(Scalar::String(s)) | Yaml::Representation(s, ..) => Value::String(s.to_string()),
            Yaml::Sequence(items) => Value::Array(items.iter().map(json).collect::<Result<_, _>>()?),
            Yaml::Mapping(map) => {
                Value::Object(map.iter().map(|(k, v)| Ok((key(k)?, json(v)?))).collect::<Result<_, String>>()?)
            }
            Yaml::Tagged(_, inner) => json(inner)?,
            Yaml::Alias(_) | Yaml::BadValue => return Err("an alias to no anchor".into()),
        })
    }
    fn key(node: &Yaml) -> Result<String, String> {
        match json(node)? {
            Value::String(s) => Ok(s),
            Value::Array(_) | Value::Object(_) => Err("a mapping key that is not a scalar".into()),
            scalar => Ok(scalar.to_string()),
        }
    }
    match Yaml::load_from_str(text).map_err(|e| e.to_string())?.as_slice() {
        [document] => json(document),
        [] => Ok(Value::Null),
        _ => Err("more than one YAML document".into()),
    }
}
