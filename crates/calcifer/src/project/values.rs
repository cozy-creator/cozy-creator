//! A tool field's value as a write keeps it: the one check `cozy apply` (`apply::Planning::value`) and the asset `PATCH`
//! (`assets::patched`) both make (#4042's review). What's off but usable is kept, fixed or as written, and said; only what can't be used
//! is refused, in one line. Values only: what a mention names, and a row's own fields, are the plan's to judge.
use api::calcifer::{
    node,
    tool::{EDIT, FieldType, ToolField},
};
use serde_json::{Value, json};

use super::format::canonical;

/// Why a tool can't use a value as one of its fields, in its own reader's words; None: it can, or its tool doesn't say.
pub(super) type Reads = fn(&str, &Value) -> Option<String>;

/// What reads `tool`'s presets: the Edit's cut (`tools::unusable`), Calcifer's own. Another tool's aren't Calcifer's to judge.
pub(super) fn reads_of(tool: &str) -> Reads {
    match node::plain(tool.trim()).trim_start_matches("_/") == EDIT {
        true => super::tools::unusable,
        false => |_, _| None,
    }
}

/// `value` of `field` as it's kept, with what's said of it, or why it can't be.
pub(super) fn checked(field: &ToolField, value: &Value, reads: Reads) -> Result<(Value, Vec<String>), String> {
    if value.is_null() {
        return Ok((Value::Null, vec![]));
    }
    let not = |what: &str| format!("{what}, not {value}");
    let kept = |value: &Value| Ok((value.clone(), vec![]));
    match field.ty {
        FieldType::Words | FieldType::Prompt => if value.is_string() { kept(value) } else { Err(not("words")) },
        FieldType::Toggle => if value.is_boolean() { kept(value) } else { Err(not("true or false")) },
        FieldType::Number => number(field, value).ok_or_else(|| not("a number")),
        FieldType::Choice => choice(field, value, reads),
        FieldType::Shape => {
            let ratio = value.as_str().filter(|said| said.split_once(':').is_some_and(|(w, h)| w.parse::<f64>().is_ok_and(|w| w > 0.0) && h.parse::<f64>().is_ok_and(|h| h > 0.0)));
            match (ratio, &field.ratios) {
                (Some(said), Some(ratios)) if !ratios.iter().any(|ratio| canonical(ratio) == canonical(value)) => {
                    let words: Vec<String> = ratios.iter().map(|ratio| ratio.as_str().unwrap_or_default().to_owned()).collect();
                    Ok((value.clone(), vec![format!("{said} isn't a ratio it lists ({}): a render clamps it to one, and says so", or_list(&words))]))
                }
                (Some(_), _) => kept(value),
                (None, _) => Err(not("a ratio, width:height, quoted: \"9:16\"")),
            }
        }
        FieldType::Seed => if value.as_u64().is_some() { kept(value) } else { Err(not("a whole number, or null for a new one each render")) },
        FieldType::Slot | FieldType::Group | FieldType::Timeline => kept(value),
    }
}

/// A number held to its bounds, whole if it must be, each fix said.
fn number(field: &ToolField, value: &Value) -> Option<(Value, Vec<String>)> {
    let mut number = value.as_f64()?;
    let mut said = vec![];
    let unit = field.unit.as_ref().map_or_else(String::new, |unit| format!(" {unit}"));
    let shown = |n: f64| if n.fract() == 0.0 { format!("{n:.0}") } else { n.to_string() };
    if field.integer && number.fract() != 0.0 {
        said.push(format!("{} is whole, so {}", shown(number), shown(number.round())));
        number = number.round();
    }
    if let Some(max) = field.max.filter(|max| number > *max) {
        said.push(format!("{} is over {}{unit}, so {}", shown(number), shown(max), shown(max)));
        number = max;
    }
    if let Some(min) = field.min.filter(|min| number < *min) {
        said.push(format!("{} is under {}{unit}, so {}", shown(number), shown(min), shown(min)));
        number = min;
    }
    Some((if number.fract() == 0.0 && number.abs() < 9e15 { json!(number as i64) } else { json!(number) }, said))
}

/// One of its options: the option itself. Presets also take a number within a thousandth of one as that one (29.97 is NTSC's
/// 30000/1001), and keep what their tool can use, said; anything else is refused.
fn choice(field: &ToolField, value: &Value, reads: Reads) -> Result<(Value, Vec<String>), String> {
    let options = field.options.as_deref().unwrap_or_default();
    let near = |option: &&Value| canonical(option) == canonical(value) || field.presets && value.as_f64().zip(option.as_f64()).is_some_and(|(a, b)| (a - b).abs() < 1e-3);
    if let Some(option) = options.iter().find(near) {
        return Ok((option.clone(), vec![]));
    }
    let shown = |option: &Value| option.as_str().map_or_else(|| option.to_string(), str::to_owned);
    if field.presets {
        let said = |option: &Value| field.words.as_ref().and_then(|words| words.get(&shown(option)).cloned()).unwrap_or_else(|| shown(option));
        if let Some(why) = reads(&field.field, value) {
            return Err(format!("{} {} {why}", field.field, shown(value)));
        }
        let listed: Vec<String> = options.iter().map(said).collect();
        return Ok((value.clone(), vec![format!("{} {} isn't one of {}'s presets ({}), so it's used as it is", field.field, shown(value), field.label, or_list(&listed))]));
    }
    let words: Vec<String> = options.iter().map(shown).collect();
    let fix = value.as_str().and_then(|said| node::near(said, words.iter().map(String::as_str))).map_or_else(String::new, |near| format!(". Did you mean {near}?"));
    Err(format!("{}, not {value}{fix}", or_list(&words)))
}

/// `a, b or c`.
pub(super) fn or_list(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}
