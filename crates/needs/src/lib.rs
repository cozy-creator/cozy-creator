//! Which of a function's models a request needs (cl-1103; Paul, 11-16: "plan, without having to install packages and run
//! arbitrary code"). A package says it in its interface: on a model slot or a called function, `needs` is one CEL boolean
//! expression over `request`, the request as JSON with each field it leaves out at its declared default. Calcifer (a quote, what a
//! box prepares) and TensorD (what a call fetches before it starts) read it here, with one evaluator. A slot with no rule is always
//! needed, and one whose rule gives no answer for a request is as one with none.
#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use api::interface::{Field, Struct, Type};
use cel::common::ast::{operators, CallExpr, ComprehensionExpr, EntryExpr, Expr, IdedExpr, LiteralValue};
use serde::Deserialize as _;
use serde_json::{Map, Value};

/// The one name a rule reads.
const REQUEST: &str = "request";
/// The longest rule, as the Runtime's describer holds one (`MAX_NEEDS_RULE`).
const MAX_RULE: usize = 1024;
/// List tests (`exists`, `all`, …) one inside another: a rule takes at most a request's length squared.
const MAX_NESTED: usize = 2;
/// What a rule may do, said where it does something else.
const MAY: &str = "a rule has `&&`, `||`, `!`, comparisons, `in`, `size()`, `has()`, and `exists`, `all`, `exists_one` over a list";

/// One call's answers: each ruled model slot (by its parameter) or called function (by its name) of its function, by what its
/// rule says of the request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answers {
    /// Its rule holds: the call waits on its weights.
    pub needed: BTreeSet<String>,
    /// Its rule doesn't hold: nothing of it is fetched for the call.
    pub unneeded: BTreeSet<String>,
    /// Its rule gave no answer, and why: it is as one with no rule.
    pub unanswered: Vec<(String, String)>,
}

/// Each rule of an interface's entrypoint: a model slot's by its parameter, a called function's by its name.
pub fn rules(entry: &Value) -> Vec<(String, String)> {
    let slots = api::interface::models(entry).iter().filter_map(|slot| {
        let parameter = api::interface::slot_path(slot)?.rsplit_once(".models.")?.1;
        Some((parameter.to_owned(), slot["needs"].as_str()?.to_owned()))
    });
    let calls = (entry["calls"].as_array().into_iter().flatten())
        .filter_map(|call| Some((call["function"].as_str()?.to_owned(), call["needs"].as_str()?.to_owned())));
    slots.chain(calls).collect()
}

/// What `entry`'s rules say of the request `sent`; one sent as nothing is its defaults.
pub fn answers(entry: &Value, sent: &Value) -> Answers {
    let mut answers = Answers::default();
    let rules = rules(entry);
    if rules.is_empty() {
        return answers;
    }
    let request = match Struct::deserialize(&entry["request"]) {
        Ok(request) => filled(&request, sent),
        Err(error) => {
            answers.unanswered = rules.into_iter().map(|(name, _)| (name, format!("its request can't be read: {error}"))).collect();
            return answers;
        }
    };
    for (name, rule) in rules {
        match holds(&rule, &request) {
            Ok(true) => answers.needed.insert(name),
            Ok(false) => answers.unneeded.insert(name),
            Err(why) => {
                answers.unanswered.push((name, why));
                continue;
            }
        };
    }
    answers
}

/// Each rule of the interface `document` that can't be read against its function's request, in words: a package with one isn't
/// published, installed or run. A document with no rules has none.
pub fn faults(document: &Value) -> Vec<String> {
    let mut faults = vec![];
    for entry in api::interface::entrypoints(document) {
        let rules = rules(entry);
        if rules.is_empty() {
            continue;
        }
        let function = api::interface::name(entry).unwrap_or_default();
        let request = match Struct::deserialize(&entry["request"]) {
            Ok(request) => request,
            Err(error) => {
                faults.push(format!("{function}: its request can't be read, so its rules can't either: {error}"));
                continue;
            }
        };
        for (name, rule) in rules {
            if let Err(why) = check(&rule, &request) {
                faults.push(format!("{function}: the rule for {name}, `{rule}`: {why}"));
            }
        }
    }
    faults
}

/// `sent` as a rule reads it: each field it leaves out at its declared default, at any depth, and a prompt sent as words alone as
/// its text with no references. A field with no default that isn't sent stays out.
pub fn filled(request: &Struct, sent: &Value) -> Value {
    match sent {
        Value::Object(members) => Value::Object(fields(&request.fields, members)),
        _ => Value::Object(fields(&request.fields, &Map::new())),
    }
}

fn fields(declared: &[Field], sent: &Map<String, Value>) -> Map<String, Value> {
    let mut filled = sent.clone();
    for field in declared {
        if let Some(held) = sent.get(&field.name).or(field.default.as_ref()) {
            filled.insert(field.name.clone(), typed(&field.ty, held));
        }
    }
    filled
}

fn typed(ty: &Type, held: &Value) -> Value {
    match (ty, held) {
        (Type::Struct(inner) | Type::Pickup(inner), Value::Object(members)) => Value::Object(fields(&inner.fields, members)),
        (Type::Media { extras, .. }, Value::Object(members)) => Value::Object(fields(&extras.fields, members)),
        (Type::List(item), Value::Array(items)) => Value::Array(items.iter().map(|held| typed(item, held)).collect()),
        (Type::Map(item), Value::Object(members)) => {
            Value::Object(members.iter().map(|(name, held)| (name.clone(), typed(item, held))).collect())
        }
        (Type::Prompt(_), Value::String(text)) => serde_json::json!({"text": text, "references": []}),
        (Type::Prompt(_), Value::Object(members)) => {
            let mut prompt = members.clone();
            prompt.entry("text").or_insert_with(|| Value::String(String::new()));
            prompt.entry("references").or_insert_with(|| Value::Array(vec![]));
            Value::Object(prompt)
        }
        (Type::Union { .. }, Value::Null) => Value::Null,
        // `T | None`, set: as `T`.
        (Type::Union { .. }, held) if !std::ptr::eq(ty.plain(), ty) => typed(ty.plain(), held),
        (_, held) => held.clone(),
    }
}

/// Whether `rule` holds for `request` (`filled`). Why not, when it gives no answer: it can't be read, it reads what the request
/// doesn't hold, or it gives something other than true or false.
pub fn holds(rule: &str, request: &Value) -> Result<bool, String> {
    let program = compiled(rule)?;
    let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut context = cel::Context::default();
        context.add_variable_from_value(REQUEST, value(request));
        program.execute(&context)
    }));
    match answer {
        Ok(Ok(cel::Value::Bool(holds))) => Ok(holds),
        Ok(Ok(other)) => Err(format!("it gives {}, not true or false", named(&other))),
        Ok(Err(error)) => Err(first_line(&error.to_string())),
        Err(_) => Err("it can't be evaluated".into()),
    }
}

/// Checks `rule` against the request it reads: it is CEL, it reads `request` alone, every field it names is one the request
/// declares, and it gives true or false. Why not, in words.
pub fn check(rule: &str, request: &Struct) -> Result<(), String> {
    let program = compiled(rule)?;
    match (Checker { request: &request.fields }).of(program.expression(), &[], 0)? {
        Kind::Bool | Kind::Any => Ok(()),
        other => Err(format!("it gives {}, not true or false", other.said())),
    }
}

fn compiled(rule: &str) -> Result<cel::Program, String> {
    if rule.len() > MAX_RULE {
        return Err(format!("it is longer than {MAX_RULE} characters"));
    }
    // The parser is a dependency's, reading a package's text: a panic in it is that text's fault, not this process's end.
    match std::panic::catch_unwind(|| cel::Program::compile(rule)) {
        Ok(Ok(program)) => Ok(program),
        Ok(Err(errors)) => Err(format!("it isn't CEL: {}", first_line(&errors.to_string()))),
        Err(_) => Err("it isn't CEL".into()),
    }
}

fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default().to_owned()
}

/// A request's JSON as CEL holds it: whole numbers as `int`, so `request.steps == 8` compares as written.
fn value(json: &Value) -> cel::Value {
    match json {
        Value::Null => cel::Value::Null,
        Value::Bool(held) => cel::Value::Bool(*held),
        Value::Number(number) => match (number.as_i64(), number.as_f64()) {
            (Some(whole), _) => cel::Value::Int(whole),
            (None, Some(real)) => cel::Value::Float(real),
            (None, None) => cel::Value::Null,
        },
        Value::String(text) => cel::Value::String(Arc::new(text.clone())),
        Value::Array(items) => cel::Value::List(Arc::new(items.iter().map(value).collect())),
        Value::Object(members) => {
            let held: HashMap<cel::objects::Key, cel::Value> =
                members.iter().map(|(name, member)| (cel::objects::Key::String(Arc::new(name.clone())), value(member))).collect();
            cel::Value::Map(cel::objects::Map { map: Arc::new(held) })
        }
    }
}

fn named(value: &cel::Value) -> &'static str {
    match value {
        cel::Value::List(_) => "a list",
        cel::Value::Map(_) => "a group of fields",
        cel::Value::Int(_) | cel::Value::UInt(_) | cel::Value::Float(_) => "a number",
        cel::Value::String(_) => "text",
        cel::Value::Null => "nothing",
        _ => "something else",
    }
}

/// What a part of a rule is, as far as the request's schema says.
#[derive(Clone, Debug)]
enum Kind<'a> {
    Bool,
    Int,
    Float,
    Text,
    Null,
    List(Box<Kind<'a>>),
    Map(Box<Kind<'a>>),
    Fields(&'a [Field]),
    /// A file with its description, and its version's own fields.
    Media(&'a [Field]),
    Prompt,
    /// It, or nothing.
    Optional(Box<Kind<'a>>),
    /// The schema doesn't say: its evaluation answers.
    Any,
}

impl Kind<'_> {
    fn said(&self) -> &'static str {
        match self {
            Kind::Bool => "true or false",
            Kind::Int => "a whole number",
            Kind::Float => "a number",
            Kind::Text => "text",
            Kind::Null => "nothing",
            Kind::List(_) => "a list",
            Kind::Map(_) => "a map",
            Kind::Fields(_) => "a group of fields",
            Kind::Media(_) => "a file",
            Kind::Prompt => "a prompt",
            Kind::Optional(_) => "a value that may be unset (compare it with `!= null`)",
            Kind::Any => "a value",
        }
    }
}

fn kind(ty: &Type) -> Kind<'_> {
    match ty {
        Type::Bool => Kind::Bool,
        Type::Int | Type::Seed => Kind::Int,
        Type::Float => Kind::Float,
        Type::Str | Type::Shape => Kind::Text,
        Type::Null => Kind::Null,
        Type::Literal(members) => literal(members),
        Type::List(item) => Kind::List(Box::new(kind(item))),
        Type::Map(item) => Kind::Map(Box::new(kind(item))),
        Type::Struct(inner) | Type::Pickup(inner) => Kind::Fields(&inner.fields),
        Type::Media { extras, .. } => Kind::Media(&extras.fields),
        Type::Prompt(_) => Kind::Prompt,
        Type::Union { branches, .. } => {
            let mut set = branches.iter().filter(|branch| **branch != Type::Null);
            match (set.next(), set.next()) {
                (Some(one), None) if branches.len() > 1 => Kind::Optional(Box::new(kind(one))),
                (Some(one), None) => kind(one),
                _ => Kind::Any,
            }
        }
        Type::Asset(_) | Type::Tree | Type::Model | Type::Unknown => Kind::Any,
    }
}

fn literal(members: &[Value]) -> Kind<'static> {
    if members.iter().all(Value::is_boolean) {
        Kind::Bool
    } else if members.iter().all(|member| member.is_i64() || member.is_u64()) {
        Kind::Int
    } else if members.iter().all(Value::is_number) {
        Kind::Float
    } else if members.iter().all(Value::is_string) {
        Kind::Text
    } else {
        Kind::Any
    }
}

/// `held`'s member `field`.
fn member<'a>(held: &Kind<'a>, field: &str) -> Result<Kind<'a>, String> {
    let among = |fields: &'a [Field]| fields.iter().find(|declared| declared.name == field).map(|declared| kind(&declared.ty));
    let absent = |has: Vec<&str>| format!("it has no field `{field}`; it has {}", has.join(", "));
    match held {
        Kind::Fields(fields) => {
            let fields: &'a [Field] = *fields;
            among(fields).ok_or_else(|| absent(fields.iter().map(|declared| declared.name.as_str()).collect()))
        }
        Kind::Media(extras) => {
            let extras: &'a [Field] = *extras;
            match field {
                "file" | "description" | "metadata" => Ok(Kind::Any),
                _ => among(extras).ok_or_else(|| {
                    absent(["file", "description", "metadata"].into_iter().chain(extras.iter().map(|declared| declared.name.as_str())).collect())
                }),
            }
        }
        Kind::Prompt => match field {
            "text" => Ok(Kind::Text),
            "references" => Ok(Kind::List(Box::new(Kind::Any))),
            _ => Err(absent(vec!["text", "references"])),
        },
        Kind::Optional(inner) => member(&**inner, field),
        Kind::Map(item) => Ok((**item).clone()),
        Kind::Any => Ok(Kind::Any),
        other => Err(format!("it is {}, which has no fields", other.said())),
    }
}

/// A name a list test binds, and what it holds.
type Name<'a> = (String, Kind<'a>);

struct Checker<'a> {
    request: &'a [Field],
}

impl<'a> Checker<'a> {
    /// What `node` is, with `names` bound, `nested` list tests deep.
    fn of(&self, node: &IdedExpr, names: &[Name<'a>], nested: usize) -> Result<Kind<'a>, String> {
        match &node.expr {
            Expr::Literal(literal) => Ok(match literal {
                LiteralValue::Boolean(_) => Kind::Bool,
                LiteralValue::Int(_) | LiteralValue::UInt(_) => Kind::Int,
                LiteralValue::Double(_) => Kind::Float,
                LiteralValue::String(_) => Kind::Text,
                LiteralValue::Null => Kind::Null,
                LiteralValue::Bytes(_) => Kind::Any,
            }),
            Expr::Ident(name) => match names.iter().rev().find(|(bound, _)| bound == name) {
                Some((_, held)) => Ok(held.clone()),
                None if name == REQUEST => Ok(Kind::Fields(self.request)),
                None => Err(format!("`{name}` isn't known: a rule reads `{REQUEST}` alone")),
            },
            Expr::Select(select) => {
                let held = self.of(&select.operand, names, nested)?;
                let found = member(&held, &select.field).map_err(|why| format!("`{}`: {why}", spelled(&select.operand)))?;
                Ok(if select.test { Kind::Bool } else { found })
            }
            Expr::Call(call) => self.call(call, names, nested),
            Expr::Comprehension(each) => self.each(each, names, nested),
            Expr::List(list) => {
                for element in &list.elements {
                    self.of(element, names, nested)?;
                }
                Ok(Kind::List(Box::new(Kind::Any)))
            }
            Expr::Map(map) => {
                for entry in &map.entries {
                    if let EntryExpr::MapEntry(entry) = &entry.expr {
                        self.of(&entry.key, names, nested)?;
                        self.of(&entry.value, names, nested)?;
                    }
                }
                Ok(Kind::Any)
            }
            Expr::Struct(_) | Expr::Unspecified => Err(format!("it builds a message; {MAY}")),
        }
    }

    fn call(&self, call: &CallExpr, names: &[Name<'a>], nested: usize) -> Result<Kind<'a>, String> {
        let target = match &call.target {
            Some(target) => Some(self.of(target, names, nested)?),
            None => None,
        };
        let mut args = vec![];
        for arg in &call.args {
            args.push(self.of(arg, names, nested)?);
        }
        let truth = |held: &Kind<'a>, sign: &str| match held {
            Kind::Bool | Kind::Any => Ok(()),
            other => Err(format!("{sign} takes true or false, and is given {}", other.said())),
        };
        let counted = |held: &Kind<'a>| matches!(held, Kind::List(_) | Kind::Map(_) | Kind::Text | Kind::Any);
        match call.func_name.as_str() {
            operators::LOGICAL_AND | operators::LOGICAL_OR | operators::LOGICAL_NOT | operators::NOT_STRICTLY_FALSE => {
                let sign = match call.func_name.as_str() {
                    operators::LOGICAL_AND => "`&&`",
                    operators::LOGICAL_OR => "`||`",
                    _ => "`!`",
                };
                for arg in &args {
                    truth(arg, sign)?;
                }
                Ok(Kind::Bool)
            }
            operators::EQUALS
            | operators::NOT_EQUALS
            | operators::LESS
            | operators::LESS_EQUALS
            | operators::GREATER
            | operators::GREATER_EQUALS
            | operators::IN => Ok(Kind::Bool),
            operators::CONDITIONAL => {
                if let Some(first) = args.first() {
                    truth(first, "`? :`")?;
                }
                Ok(match (args.get(1), args.get(2)) {
                    (Some(Kind::Bool), Some(Kind::Bool)) => Kind::Bool,
                    _ => Kind::Any,
                })
            }
            operators::INDEX | operators::OPT_INDEX => match args.first() {
                Some(Kind::List(item) | Kind::Map(item)) => Ok((**item).clone()),
                Some(Kind::Any | Kind::Optional(_)) | None => Ok(Kind::Any),
                Some(other) => Err(format!("`[ ]` reads a list or a map, and is given {}", other.said())),
            },
            operators::ADD | operators::SUBSTRACT | operators::MULTIPLY | operators::DIVIDE | operators::MODULO | operators::NEGATE => {
                Ok(match args.first() {
                    Some(Kind::Int) => Kind::Int,
                    Some(Kind::Float) => Kind::Float,
                    Some(Kind::Text) => Kind::Text,
                    _ => Kind::Any,
                })
            }
            "size" => match target.as_ref().or(args.first()) {
                Some(held) if counted(held) => Ok(Kind::Int),
                Some(Kind::Optional(inner)) if counted(&**inner) => Ok(Kind::Int),
                Some(other) => Err(format!("`size()` counts a list, a map or text, and is given {}", other.said())),
                None => Err("`size()` counts a list, a map or text".into()),
            },
            "contains" | "startsWith" | "endsWith" | "bool" => Ok(Kind::Bool),
            "int" | "uint" => Ok(Kind::Int),
            "double" => Ok(Kind::Float),
            "string" => Ok(Kind::Text),
            "min" | "max" => Ok(Kind::Any),
            other => Err(format!("`{other}` isn't something a rule may call; {MAY}")),
        }
    }

    /// A list test (`exists`, `all`, `exists_one`, `map`, `filter`), which the parser spells as a loop over its list.
    fn each(&self, each: &ComprehensionExpr, names: &[Name<'a>], nested: usize) -> Result<Kind<'a>, String> {
        if nested == MAX_NESTED {
            return Err(format!("it tests lists more than {MAX_NESTED} deep"));
        }
        let item = match self.of(&each.iter_range, names, nested)? {
            Kind::List(item) => *item,
            Kind::Map(_) => Kind::Text,
            Kind::Any | Kind::Optional(_) => Kind::Any,
            other => return Err(format!("`exists`, `all` and the like read a list, and are given {}", other.said())),
        };
        let mut inner = names.to_vec();
        match &each.iter_var2 {
            // Two names: the first is each item's place, the second the item.
            Some(second) => {
                inner.push((each.iter_var.clone(), Kind::Any));
                inner.push((second.clone(), item));
            }
            None => inner.push((each.iter_var.clone(), item)),
        }
        let kept = self.of(&each.accu_init, &inner, nested + 1)?;
        inner.push((each.accu_var.clone(), kept));
        self.of(&each.loop_cond, &inner, nested + 1)?;
        self.of(&each.loop_step, &inner, nested + 1)?;
        self.of(&each.result, &inner, nested + 1)
    }
}

/// A part of a rule as its author wrote it, where it is a name and its fields: `request.beats`. Else `it`.
fn spelled(node: &IdedExpr) -> String {
    match &node.expr {
        Expr::Ident(name) => name.clone(),
        Expr::Select(select) if !select.test => format!("{}.{}", spelled(&select.operand), select.field),
        _ => "it".into(),
    }
}
