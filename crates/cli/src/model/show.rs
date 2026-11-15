//! `cozy model show` and `cozy lora show`: what each lane's checkpoint holds, read from its header alone through Calcifer
//! (CALCIFER-API.md "Models"): its components' classes, configs, run data, tensors and their size, and the base or recipe it records.
use std::process::ExitCode;

use api::calcifer::hub::{ComponentHeader, LaneHeader, ModelHeaders};
use api::error::{Error, Kind};
use api::model_ref::ModelRef;
use serde_json::Value;

use super::Show;
use crate::Context;
use crate::calcifer_client::path;
use crate::output::{self, Record, Table};

pub(crate) async fn show(ctx: &Context, args: Show, noun: &str) -> Result<ExitCode, Error> {
    let (model, release, checkpoint) = match ModelRef::parse(&args.model)? {
        ModelRef::Hub { model, release } => (model, release, None),
        ModelRef::Checkpoint { model, checkpoint } => (model, None, Some(checkpoint.to_string())),
        _ => {
            return Err(Error::new(Kind::Usage, "model_ref_invalid", format!("{} names no Hub model", args.model))
                .next(format!("cozy {noun} show <org/name>[@release]")));
        }
    };
    let mut query = vec![("hub", ctx.hub().to_string())];
    query.extend(release.map(|release| ("release", release)));
    query.extend(checkpoint.map(|checkpoint| ("checkpoint", checkpoint)));
    query.extend(args.lane.map(|lane| ("lane", lane)));
    if ctx.full {
        query.push(("full", "true".into()));
    }
    let at = path(&["v1", "models", &model.org, &model.name, "headers"]);
    let shown: ModelHeaders = ctx.calcifer().await?.get_query(&at, &query).await?;
    if ctx.json {
        output::json(&shown);
        return Ok(ExitCode::SUCCESS);
    }
    let name = shown.release.as_ref().map_or_else(|| shown.model.clone(), |release| format!("{}@{release}", shown.model));
    let private = if shown.visibility == Some(api::model_ref::ReleaseVisibility::Private) { " (private)" } else { "" };
    anstream::println!("{name}{private}{}", shown.family.as_ref().map_or_else(String::new, |family| format!(" · {family}")));
    if let (Some(release), Some(newer)) = (&shown.release, &shown.resolves_to) {
        output::say(ctx, &format!("A run naming {release} takes {newer}, its newest fix."));
    }
    for lane in &shown.lanes {
        anstream::println!();
        print_lane(ctx, lane);
    }
    Ok(ExitCode::SUCCESS)
}

fn print_lane(ctx: &Context, lane: &LaneHeader) {
    let kind = match (&lane.kind, &lane.family) {
        (Some(kind), Some(family)) => format!("{}, {family}", word(kind)),
        (Some(kind), None) => word(kind).into(),
        (None, _) => "unknown".into(),
    };
    let mut record = Record::new()
        .field("lane", &lane.lane)
        .field("checkpoint", &lane.checkpoint)
        .field("header", format!("{} ({})", lane.header, output::bytes(lane.header_bytes)))
        .field("kind", kind);
    if let Some(pipeline) = &lane.pipeline {
        record = record.field("pipeline", class(pipeline));
    }
    for (key, value) in &lane.records {
        record = record.field(key, recorded(key, value));
    }
    record.print(ctx);
    let mut table = Table::new(ctx, ["component", "class", "config", "tensors", "size", "files"]);
    for component in &lane.components {
        let size = if component.tensors == 0 { "—".into() } else { output::bytes(component.bytes) };
        table = table.row([component.name.clone(), class(&component.class), config(component), tensors(component), size, files(component)]);
    }
    table.print();
    for component in lane.components.iter().filter(|component| component.config.is_some()) {
        let config = serde_json::to_string_pretty(&component.config).unwrap_or_default();
        anstream::println!("{} config:\n{config}", component.name);
    }
}

fn word(kind: &api::calcifer::hub::ModelContentKind) -> &'static str {
    match kind {
        api::calcifer::hub::ModelContentKind::Lora => "LoRA",
        api::calcifer::hub::ModelContentKind::Weights => "weights",
        api::calcifer::hub::ModelContentKind::Component => "component",
    }
}

/// `library.Name version`; `—` for an unknown class.
fn class([library, version, name]: &[String; 3]) -> String {
    match (library.is_empty(), version.is_empty()) {
        (true, _) => "—".into(),
        (false, true) => format!("{library}.{name}"),
        (false, false) => format!("{library}.{name} {version}"),
    }
}

fn config(component: &ComponentHeader) -> String {
    let keys = match component.config_keys.len() {
        0 => "none".to_owned(),
        1 => "1 key".to_owned(),
        n => format!("{n} keys"),
    };
    match &component.config_file {
        Some(file) if !component.config_keys.is_empty() => format!("{keys} ({file})"),
        _ => keys,
    }
}

fn tensors(component: &ComponentHeader) -> String {
    let Some(lora) = &component.lora else { return component.tensors.to_string() };
    let alpha = match lora.lora_alpha {
        Some(alpha) => format!("α {}/{}, lora_alpha {alpha}", lora.alphas, lora.modules),
        None => format!("α {}/{}", lora.alphas, lora.modules),
    };
    format!("{} ({} modules, {alpha})", component.tensors, lora.modules)
}

fn files(component: &ComponentHeader) -> String {
    match component.assets.is_empty() {
        true => "—".into(),
        false => component.assets.iter().map(|(path, bytes)| format!("{path} {}", output::bytes(*bytes))).collect::<Vec<_>>().join(", "),
    }
}

fn text<'v>(value: &'v Value, key: &str) -> &'v str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn texts(value: &Value, key: &str) -> Vec<String> {
    match value.get(key) {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        _ => vec![],
    }
}

/// `model@release lane (header …)`, as a base names itself.
fn base(base: &Value) -> String {
    let release = text(base, "release");
    let named = match release.is_empty() { true => text(base, "model").to_owned(), false => format!("{}@{release}", text(base, "model")) };
    let header = text(base, "header");
    let header = if header.is_empty() { String::new() } else { format!("(header {header})") };
    [named.as_str(), text(base, "lane"), header.as_str()].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")
}

/// A header record as one line: the base it names and what it took, the recipe, the family or each component's adapter.
fn recorded(key: &str, value: &Value) -> String {
    match key {
        "inherited" => match value.get("from") {
            Some(Value::Array(bases)) => (bases.iter())
                .map(|from| {
                    let took: Vec<String> = texts(from, "components").into_iter().chain(texts(from, "files")).collect();
                    format!("{}: {}", base(from.get("base").unwrap_or(&Value::Null)), took.join(", "))
                })
                .collect::<Vec<_>>()
                .join("; "),
            _ => value.to_string(),
        },
        "composed" => format!("{}: {}", base(value.get("base").unwrap_or(&Value::Null)), texts(value, "components").join(", ")),
        "provenance" => {
            let source = text(value, "source");
            let from = match source.is_empty() { true => String::new(), false => format!(", from {source}") };
            format!("{} {} {}{from}", text(value, "package"), text(value, "release"), text(value, "recipe"))
        }
        "family" => format!("{} {}", text(value, "name"), text(value, "form")).trim().to_owned(),
        "adapter" => match value {
            Value::Object(adapters) => (adapters.iter())
                .map(|(component, adapter)| {
                    let trained = adapter.get("trained_on").map(|on| format!(", trained on {}", text(on, "component"))).unwrap_or_default();
                    format!("{component}: {} {}{trained}", text(adapter, "family"), text(adapter, "component"))
                })
                .collect::<Vec<_>>()
                .join("; "),
            _ => value.to_string(),
        },
        _ => match value {
            Value::Array(items) => items.iter().map(|item| item.as_str().map_or_else(|| item.to_string(), str::to_owned)).collect::<Vec<_>>().join(", "),
            _ => value.to_string(),
        },
    }
}
