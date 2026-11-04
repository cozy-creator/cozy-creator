//! A tool: a public function that makes a picture, a video or a sound, as `GET /v1/tools` lists it. What it makes comes from its
//! output, and its box from what it takes: the typed `fields` of an adapter that speaks for it, else its own inputs. Nothing here
//! knows a model.
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    interface::{Callable, Constraints, Field, Interface, Struct, Type, Wire},
    names::shown,
};

/// The kinds of file a node can be.
const MEDIA: [&str; 3] = ["image", "video", "audio"];
/// What a run sets itself, so no box asks for it.
const NOT_ASKED: [&str; 1] = ["seed"];

/// `GET /v1/tools`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tools {
    pub tools: Vec<Tool>,
    /// Why the list holds only what's installed: the Hub didn't answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// What a node names as its `tool`: `qwen-image-2/generate_turbo`, `acme/sketchlab/draw`.
    pub id: String,
    pub package: String,
    pub function: String,
    /// The release this box was read from: the one its machine holds, else the Hub's newest.
    pub release: String,
    pub label: String,
    /// What a node of it is: `image`, `video` or `audio`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub controls: Vec<Control>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlType {
    Text,
    Beats,
    Shape,
    Range,
    Choice,
    Toggle,
    File,
}

/// One thing a box asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Control {
    /// What a node's fields call it.
    pub field: String,
    #[serde(rename = "type")]
    pub ty: ControlType,
    pub label: String,
    /// `choice`, and a `shape` that allows only these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<Value>>,
    /// `range`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// `file`: what it takes, `image`, `video` or `audio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<String>,
    /// `file`: several, in order.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multiple: bool,
    /// `beats`: the longest a beat can be, the package's own limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

/// What `callable` makes: the first field of its output that is a picture, a video or a sound. None: it makes no media.
pub fn output_kind(callable: &Callable) -> Option<&str> {
    callable.result.fields.iter().find_map(|field| field.ty.media().map(|(kind, _)| kind).filter(|kind| MEDIA.contains(kind)))
}

/// Every tool of a package's interface: what a person runs of it that makes media.
pub fn tools_of(package: &str, release: &str, interface: &Interface) -> Vec<Tool> {
    let callables = interface.public().into_iter().filter_map(|name| interface.callables.iter().find(|c| c.name == name));
    callables.filter_map(|callable| tool_of(package, release, interface, callable)).collect()
}

fn tool_of(package: &str, release: &str, interface: &Interface, callable: &Callable) -> Option<Tool> {
    let kind = output_kind(callable)?;
    let package = shown(package);
    let box_of = interface.adapter(kind, Some(&callable.name)).and_then(Callable::typed_fields);
    let controls = match box_of {
        Some(fields) => controls(fields, None),
        None => controls(&callable.request, Some(callable)),
    };
    Some(Tool {
        id: format!("{package}/{}", callable.name),
        label: label(&package, &callable.name),
        package,
        function: callable.name.clone(),
        release: release.to_owned(),
        kind: kind.to_owned(),
        description: callable.description.clone(),
        controls,
    })
}

/// `Qwen Image 2 Turbo`, `Minimax H3 Upscale`: a package's words and what the function adds to `generate`.
fn label(package: &str, function: &str) -> String {
    let mut out = String::new();
    for word in package.rsplit('/').next().unwrap_or(package).split(['-', '_']).filter(|word| !word.is_empty()) {
        let _ = write!(out, "{} ", capitalized(word));
    }
    let added = function.strip_prefix("generate").map_or(function, |rest| rest.trim_start_matches('_'));
    out.push_str(&capitalized(&added.replace('_', " ")));
    out.trim_end().to_owned()
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

fn controls(fields: &Struct, callable: Option<&Callable>) -> Vec<Control> {
    fields.fields.iter().filter_map(|field| control(field, callable)).collect()
}

/// A field as its control. None: it isn't one, because a run sets it, mentions fill it, or a person couldn't set it by hand.
fn control(field: &Field, callable: Option<&Callable>) -> Option<Control> {
    if NOT_ASKED.contains(&field.name.as_str()) || callable.is_some_and(|callable| callable.is_assets(&field.name)) {
        return None;
    }
    let ty = optional(&field.ty);
    let mut control = Control {
        // On the wire a project's text is `instructions`.
        field: if field.name == "instructions" { "prompt".to_owned() } else { field.name.clone() },
        ty: ControlType::Text,
        label: field.extra.get("label").and_then(Value::as_str).map_or_else(|| capitalized(&words(&field.name)), str::to_owned),
        options: None,
        min: None,
        max: None,
        step: None,
        accept: None,
        multiple: false,
        max_duration: None,
        default: callable.and_then(|callable| callable.invocable.defaults.get(&format!("request/{}", field.name))).or(field.default.as_ref()).filter(|value| !value.is_null()).cloned(),
        required: field.wire == Wire::Required,
    };
    if let Some(accept) = field.extra.get("accept").and_then(Value::as_str) {
        control.ty = ControlType::File;
        control.accept = Some(accept.to_owned());
        return Some(control);
    }
    if field.name == "aspect_ratio" {
        control.ty = ControlType::Shape;
        control.options = choices(ty);
        return Some(control);
    }
    match ty {
        Type::Str => {}
        Type::Literal(_) => {
            control.ty = ControlType::Choice;
            control.options = choices(ty);
        }
        Type::Bool => control.ty = ControlType::Toggle,
        Type::Int | Type::Float => {
            let (min, max) = bounds(&field.constraints)?;
            control.ty = ControlType::Range;
            (control.min, control.max) = (Some(min), Some(max));
            control.step = field.constraints.multiple_of.or((*ty == Type::Int).then_some(1.0));
        }
        Type::Asset(kind) if MEDIA.contains(&kind.as_str()) => {
            control.ty = ControlType::File;
            control.accept = Some(kind.clone());
        }
        Type::List(item) => match &**item {
            Type::Asset(kind) if MEDIA.contains(&kind.as_str()) => {
                control.ty = ControlType::File;
                control.accept = Some(kind.clone());
                control.multiple = true;
            }
            Type::Struct(item) => {
                let beat = |name: &str| item.fields.iter().find(|member| member.name == name);
                let duration = beat("duration").filter(|duration| matches!(optional(&duration.ty), Type::Int | Type::Float))?;
                beat("description").filter(|description| *optional(&description.ty) == Type::Str)?;
                control.ty = ControlType::Beats;
                control.max_duration = duration.extra.get("max_duration").and_then(Value::as_f64).or(duration.constraints.le);
            }
            _ => return None,
        },
        _ => return None,
    }
    Some(control)
}

fn words(name: &str) -> String {
    name.replace('_', " ")
}

/// `ty`, or what it is when it's there: `T | None` is `T`.
fn optional(ty: &Type) -> &Type {
    match ty {
        Type::Union { branches, .. } if branches.len() == 2 && branches.contains(&Type::Null) => branches.iter().find(|branch| **branch != Type::Null).unwrap_or(ty),
        _ => ty,
    }
}

fn choices(ty: &Type) -> Option<Vec<Value>> {
    let Type::Literal(members) = ty else { return None };
    Some(members.iter().filter(|member| !member.is_null()).cloned().collect())
}

/// A number's two ends, if it has them.
fn bounds(constraints: &Constraints) -> Option<(f64, f64)> {
    Some((constraints.ge.or(constraints.gt)?, constraints.le.or(constraints.lt)?))
}
