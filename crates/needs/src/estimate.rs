//! What a request costs (cl-1286; Paul, 11-18: "all we need is something so that calcifer or the planner more generally can be like
//! 'how long will X workload take with model Y on GPU G' and it can get that from a formula, with the formula hand-built per package
//! based on measured times we store in the same benchmark"). A function says it in its interface, beside its `needs`: `estimate` holds
//! its size variables (`size`: a name to a CEL expression over `request`, a number) and any of `OUTPUTS` (a CEL expression over
//! `request`, `size` and `machine`, a number). The expressions call three helpers, and nothing else of their own:
//! - `measured(machine)`, `measured(machine, "function")`: the function's own benchmark row for this machine (another function of its
//!   package, named): `warm_ms`, `cold_ms`, `peak_device_bytes`, `peak_host_bytes`, `budget_bytes`, `request` (its size), `gpu`,
//!   `lane` and `scale` (`Row`). No value where nothing measured relates to the machine; `measured(machine, "function", {…})`, that
//!   map instead: a placeholder until a row of it exists.
//! - `pow(x, y)`: x to the power y.
//! - `sum(list)`: its numbers added; `min(x, y, …)`, `max(x, y, …)`: the least and the greatest.
//!
//! One evaluation of a request on a machine gives its size and its outputs. A function that declares no `ms` is timed by the platform's
//! own default, its nearest row scaled (`References::time`); one that declares no `size` is sized by `JobSize::of`'s guess.

use std::collections::HashMap;

use api::{
    estimate::{is_card, reference_key, Benchmark, References},
    interface::Struct,
    machine::{Basis, JobSize},
};
use serde::Deserialize as _;
use cel::extractors::Arguments;
use serde_json::{json, Map, Value};

use crate::{compiled, first_line, named, value};

/// What an `estimate` may say of a request beside its size.
pub const OUTPUTS: [&str; 4] = ["ms", "cold_ms", "peak_device_bytes", "peak_host_bytes"];
/// The helper that reads a function's benchmark rows. The others: `pow`, `sum`, `min`, `max` (`context`).
pub(crate) const MEASURED: &str = "measured";

/// A function's `estimate` as its interface declares it: its size variables and its outputs, by name, each one CEL expression.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declared {
    pub size: Vec<(String, String)>,
    pub outputs: Vec<(String, String)>,
}

impl Declared {
    pub fn of(entry: &Value) -> Self {
        Self::parse(&entry["estimate"])
    }

    /// An `estimate` declaration as written: `{"size": {name: expression}, output: expression}`.
    pub fn parse(estimate: &Value) -> Self {
        let pairs = |held: &Value| -> Vec<(String, String)> {
            (held.as_object().into_iter().flatten()).filter_map(|(name, expression)| Some((name.clone(), expression.as_str()?.to_owned()))).collect()
        };
        let outputs = pairs(estimate).into_iter().filter(|(name, _)| OUTPUTS.contains(&name.as_str())).collect();
        Declared { size: pairs(&estimate["size"]), outputs }
    }

    fn output(&self, name: &str) -> Option<&str> {
        self.outputs.iter().find(|(held, _)| held == name).map(|(_, expression)| expression.as_str())
    }
}

/// The machine an estimate reads (`machine`): a live one's, or a card the Hub sells, with no machine behind it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Card {
    /// Its GPU as its driver or the Hub names it (`NVIDIA GeForce RTX 4090`); empty: it has none, and runs on its CPU.
    pub gpu: String,
    pub gpus: u32,
    /// One GPU's memory; with no GPU, the host's.
    pub memory_bytes: u64,
    /// The lane its call runs at (`int8-convrot`); empty: unknown.
    pub lane: String,
    /// Its processor as its OS names it, its physical cores and its threads.
    pub cpu: String,
    pub cores: u32,
    pub threads: u32,
}

impl Card {
    fn value(&self) -> Value {
        json!({"gpu": self.gpu, "gpus": f64::from(self.gpus), "memory_bytes": self.memory_bytes as f64, "lane": self.lane,
            "cpu": self.cpu, "cores": f64::from(self.cores), "threads": f64::from(self.threads)})
    }
}

/// The fields `machine` holds, for the checker.
pub(crate) const MACHINE: [&str; 7] = ["gpu", "gpus", "memory_bytes", "lane", "cpu", "cores", "threads"];
/// The fields `measured(…)` gives, for the checker.
pub(crate) const ROW: [&str; 9] = ["warm_ms", "cold_ms", "peak_device_bytes", "peak_host_bytes", "budget_bytes", "request", "gpu", "lane", "scale"];

/// What a request costs on a card: its time and its peaks where known, and what they stand on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Estimated {
    pub ms: Option<f64>,
    pub cold_ms: Option<f64>,
    pub peak_device_bytes: Option<f64>,
    pub peak_host_bytes: Option<f64>,
    /// `Reference`: the card's own row; `Scaled`: another card's, or a budgeted one's; `None`: no row relates to the card.
    pub basis: Basis,
    /// It is the function's own formula's, not the platform's default.
    pub declared: bool,
    /// Each declared output that gave no value, and why.
    pub unanswered: Vec<(String, String)>,
}

/// One request as its function's estimate reads it (cl-1286): filled at its defaults, sized, and what its function declares of its
/// cost; what it costs on a machine is `Cost::on`. Built from the interface's entry (`Cost::of`) or a parsed callable's request and
/// `estimate` (`Cost::declared`), one path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cost {
    /// Its size: its function's declared variables, else `JobSize::of`'s guess.
    pub size: JobSize,
    /// Each declared size variable that gave no number for it, and why: left out of `size`.
    pub size_unanswered: Vec<(String, String)>,
    /// Its function's estimate where it can't be read against its request, in words: it is then as one that declares none (today's
    /// guessed size and time). Refused where a package is read (`estimate_faults`); a run of one read before only says so.
    pub faults: Vec<String>,
    request: Value,
    declared: Declared,
}

impl Cost {
    /// The request `sent` to the function `entry` of an interface.
    pub fn of(entry: &Value, sent: &Value) -> Self {
        match Struct::deserialize(&entry["request"]) {
            Ok(request) => Self::declared(&request, &entry["estimate"], sent),
            Err(error) => {
                let declared = Declared::parse(&entry["estimate"]);
                let why = format!("its request can't be read: {error}");
                let size_unanswered = declared.size.iter().map(|(name, _)| (name.clone(), why.clone())).collect();
                Cost { size: JobSize::of(sent), size_unanswered, faults: vec![], request: sent.clone(), declared }
            }
        }
    }

    /// The request `sent` to a function whose request is `request` and whose `estimate` is `estimate` (null: none declared).
    pub fn declared(request: &Struct, estimate: &Value, sent: &Value) -> Self {
        let (mut declared, faults) = (Declared::parse(estimate), crate::declared_faults(&Declared::parse(estimate), request));
        if !faults.is_empty() {
            declared = Declared::default();
        }
        let request = Value::Object(crate::fields(&request.fields, sent.as_object().unwrap_or(&Map::new())));
        let (mut size, mut size_unanswered) = (JobSize::default(), vec![]);
        if declared.size.is_empty() {
            size = JobSize::of(sent);
        }
        let context = context(&request, None, HashMap::new());
        for (name, expression) in &declared.size {
            match number(expression, &context) {
                Ok(n) if n > 0.0 => {
                    size.values.insert(name.clone(), n);
                }
                Ok(n) => size_unanswered.push((name.clone(), format!("it gives {n}, not a number above 0"))),
                Err(why) => size_unanswered.push((name.clone(), why)),
            }
        }
        Cost { size, size_unanswered, faults, request, declared }
    }

    /// Its function declares its own size variables.
    pub fn sized(&self) -> bool {
        !self.declared.size.is_empty()
    }

    /// Its function declares its own time.
    pub fn timed(&self) -> bool {
        self.declared.output("ms").is_some()
    }

    /// What it costs on `card`, its function (`[org/]package/function`, as `References` keys it) timed by its rows in `references`: its
    /// declared outputs; with no `ms` declared, the platform's default time (`References::time`) and its nearest row's peaks.
    pub fn on(&self, function: &str, card: &Card, references: &References) -> Estimated {
        on(&self.declared, function, &self.request, &self.size, card, references)
    }
}

fn on(declared: &Declared, function: &str, request: &Value, size: &JobSize, card: &Card, references: &References) -> Estimated {
    let own = row(references, function, card, size);
    let mut rows = HashMap::from([(String::new(), own.clone())]);
    let package = function.rsplit_once('/').map_or("", |(package, _)| package);
    for (_, expression) in &declared.outputs {
        for other in crate::measured_names(expression).into_iter().filter(|other| !other.is_empty()) {
            let key = format!("{package}/{other}");
            rows.entry(other).or_insert_with(|| row(references, &key, card, size));
        }
    }
    // A formula's time stands on the least sure of the rows it reads: a card's own, else scaled; one that reads none, on its author.
    let read = declared.output("ms").map(crate::measured_names).unwrap_or_default();
    // A row a formula reads with a placeholder may be absent: it then stands on the rows it does read. One read without a placeholder
    // and absent gives no time at all (below).
    let present: Vec<Basis> = read.iter().filter_map(|name| rows.get(name).cloned().flatten().map(|row| row.basis)).collect();
    let basis = match (read.is_empty(), present.iter().max_by_key(|basis| matches!(basis, Basis::Scaled))) {
        (true, _) => Basis::Reference,
        (false, Some(basis)) => *basis,
        (false, None) => Basis::None,
    };
    let context = context(request, Some((size, card)), rows.iter().map(|(name, row)| (name.clone(), row.as_ref().map(Row::value))).collect());
    let mut estimated = Estimated { basis, declared: declared.output("ms").is_some(), ..Estimated::default() };
    for name in OUTPUTS {
        let Some(expression) = declared.output(name) else { continue };
        match number(expression, &context) {
            Ok(n) if n >= 0.0 => *slot(&mut estimated, name) = Some(n),
            Ok(n) => estimated.unanswered.push((name.to_owned(), format!("it gives {n}, below 0"))),
            Err(why) => estimated.unanswered.push((name.to_owned(), why)),
        }
    }
    if !estimated.declared {
        let time = references.time(function, &card.gpu, size);
        estimated.ms = time.map(|(ms, _)| ms);
        estimated.basis = time.map_or(Basis::None, |(_, basis)| basis);
    }
    if let Some(row) = own.filter(|_| !estimated.declared || estimated.ms.is_some()) {
        let scaled = |ms: Option<u64>| ms.map(|ms| ms as f64 * row.scale);
        estimated.cold_ms = estimated.cold_ms.or(scaled(row.row.cold_ms));
        estimated.peak_device_bytes = estimated.peak_device_bytes.or(row.row.peak_device_bytes.map(|b| b as f64));
        estimated.peak_host_bytes = estimated.peak_host_bytes.or(row.row.peak_host_bytes.map(|b| b as f64));
    }
    if estimated.ms.is_none() {
        estimated.basis = Basis::None;
    }
    estimated
}

fn slot<'e>(estimated: &'e mut Estimated, name: &str) -> &'e mut Option<f64> {
    match name {
        "ms" => &mut estimated.ms,
        "cold_ms" => &mut estimated.cold_ms,
        "peak_device_bytes" => &mut estimated.peak_device_bytes,
        _ => &mut estimated.peak_host_bytes,
    }
}

/// A benchmark row as `measured(…)` gives it: its times times `scale`, how much longer this card takes than the row's.
#[derive(Clone, Debug)]
struct Row {
    row: Benchmark,
    scale: f64,
    basis: Basis,
}

impl Row {
    fn value(&self) -> Value {
        let mut held = Map::new();
        held.insert("warm_ms".into(), json!(self.row.warm_ms as f64 * self.scale));
        let mut put = |name: &str, n: Option<f64>| {
            if let Some(n) = n {
                held.insert(name.into(), json!(n));
            }
        };
        put("cold_ms", self.row.cold_ms.map(|ms| ms as f64 * self.scale));
        put("peak_device_bytes", self.row.peak_device_bytes.map(|b| b as f64));
        put("peak_host_bytes", self.row.peak_host_bytes.map(|b| b as f64));
        put("budget_bytes", self.row.budget_bytes.map(|b| b as f64));
        put("scale", Some(self.scale));
        held.insert("request".into(), json!(self.row.request.values));
        held.insert("gpu".into(), json!(self.row.gpu));
        held.insert("lane".into(), json!(self.row.lane));
        Value::Object(held)
    }
}

/// `measured(machine)`: `function`'s row for `card`. With a GPU: of its one-GPU rows measured on no memory budget at the card's lane
/// (else at none named, else any), the one nearest `size` on this card; with none of this card, the nearest on the card most like it,
/// its times scaled by how the two compare (`References::factor`). Where that row's peak is over the card's memory, its rows measured
/// under the budget nearest at or below that memory (else the smallest) instead, as the card runs it: memory over a card's lengthens
/// its time, never refuses it (Paul: every model runs in 6 GB). With no GPU, its CPU row (`gpus` 0) nearest `size`. None: no row
/// relates to the card.
fn row(references: &References, function: &str, card: &Card, size: &JobSize) -> Option<Row> {
    let rows: &[Benchmark] = references.rows.get(&reference_key(function)).map_or(&[], Vec::as_slice);
    if card.gpu.is_empty() {
        return nearest(at_lane(rows.iter().filter(|row| row.gpus == 0).collect(), &card.lane), size)
            .map(|row| Row { row: row.clone(), scale: 1.0, basis: Basis::Reference });
    }
    let on_card = |rows: Vec<&Benchmark>| -> Option<Row> {
        let rows = at_lane(rows, &card.lane);
        let own: Vec<&Benchmark> = rows.iter().copied().filter(|row| is_card(&card.gpu, &row.gpu)).collect();
        if let Some(row) = nearest(own, size) {
            return Some(Row { row: row.clone(), scale: 1.0, basis: Basis::Reference });
        }
        let mut cards: Vec<&str> = rows.iter().map(|row| row.gpu.as_str()).collect();
        cards.sort_unstable();
        cards.dedup();
        let (other, scale) = (cards.into_iter())
            .filter_map(|other| Some((other, references.factor(other, &card.gpu, function)?)))
            .min_by(|a, b| a.1.ln().abs().total_cmp(&b.1.ln().abs()))?;
        let row = nearest(rows.into_iter().filter(|row| row.gpu == other).collect(), size)?;
        Some(Row { row: row.clone(), scale, basis: Basis::Scaled })
    };
    let one: Vec<&Benchmark> = rows.iter().filter(|row| row.gpus == 1).collect();
    let uncapped = on_card(one.iter().copied().filter(|row| row.budget_bytes.is_none()).collect());
    let over = |row: &Row| row.row.peak_device_bytes.is_some_and(|peak| card.memory_bytes > 0 && peak > card.memory_bytes);
    if uncapped.as_ref().is_some_and(|row| !over(row)) {
        return uncapped;
    }
    let mut budgets: Vec<u64> = one.iter().filter_map(|row| row.budget_bytes).collect();
    budgets.sort_unstable();
    budgets.dedup();
    let budget = (budgets.iter().rev().find(|budget| **budget <= card.memory_bytes)).or(budgets.first());
    match budget.and_then(|budget| on_card(one.iter().copied().filter(|row| row.budget_bytes == Some(*budget)).collect())) {
        Some(squeezed) => Some(Row { basis: Basis::Scaled, ..squeezed }),
        None => uncapped,
    }
}

/// Of `rows`, those at `lane`; else those that name none; else, with no lane known, all.
fn at_lane<'r>(rows: Vec<&'r Benchmark>, lane: &str) -> Vec<&'r Benchmark> {
    let of = |lane: &str| rows.iter().copied().filter(|row| row.lane == lane).collect::<Vec<_>>();
    match (of(lane), lane.is_empty()) {
        (named, _) if !named.is_empty() => named,
        (_, false) => of(""),
        (_, true) => rows,
    }
}

/// Of `rows`, the one whose size is most like `size`: the fewest pictures apart, then the least apart by their shared variables.
fn nearest<'r>(rows: Vec<&'r Benchmark>, size: &JobSize) -> Option<&'r Benchmark> {
    let refs = size.refs.unwrap_or(0);
    let unlike = |row: &Benchmark| (row.request.refs.unwrap_or(0).abs_diff(refs), size.ratio(&row.request).ln().abs());
    rows.into_iter().min_by(|a, b| {
        let (a, b) = (unlike(a), unlike(b));
        a.0.cmp(&b.0).then(a.1.total_cmp(&b.1))
    })
}

/// An estimate's context: `request`; with a machine, `size`, `machine` and `measured`'s rows by function (`""`: its own; none: nothing
/// measured relates to it).
fn context(request: &Value, machine: Option<(&JobSize, &Card)>, rows: HashMap<String, Option<Value>>) -> cel::Context<'static, 'static> {
    let mut context = cel::Context::default();
    context.add_variable_from_value(crate::REQUEST, value(request));
    if let Some((size, card)) = machine {
        context.add_variable_from_value("size", value(&json!(size.values)));
        context.add_variable_from_value("machine", value(&card.value()));
    }
    let _ = context.add_function("pow", |Arguments(args): Arguments| -> Result<cel::Value, cel::ExecutionError> {
        match (args.first().and_then(real), args.get(1).and_then(real)) {
            (Some(x), Some(y)) => Ok(cel::Value::Float(x.powf(y))),
            _ => Err(cel::ExecutionError::function_error("pow", "pow(x, y) takes two numbers")),
        }
    });
    for (name, pick) in [("min", f64::min as fn(f64, f64) -> f64), ("max", f64::max)] {
        let _ = context.add_function(name, move |Arguments(args): Arguments| -> Result<cel::Value, cel::ExecutionError> {
            let numbers: Option<Vec<f64>> = args.iter().map(real).collect();
            match numbers.and_then(|numbers| numbers.into_iter().reduce(pick)) {
                Some(n) => Ok(cel::Value::Float(n)),
                None => Err(cel::ExecutionError::function_error(name, format!("{name}(x, y, …) takes numbers"))),
            }
        });
    }
    let _ = context.add_function("sum", |Arguments(args): Arguments| -> Result<cel::Value, cel::ExecutionError> {
        let Some(cel::Value::List(items)) = args.first() else { return Err(cel::ExecutionError::function_error("sum", "sum(list) adds a list")) };
        items.iter().map(real).sum::<Option<f64>>().map(cel::Value::Float).ok_or_else(|| cel::ExecutionError::function_error("sum", "sum(list) adds numbers"))
    });
    let _ = context.add_function("measured", move |Arguments(args): Arguments| -> Result<cel::Value, cel::ExecutionError> {
        let function = match args.get(1) {
            Some(cel::Value::String(name)) => name.to_string(),
            _ => String::new(),
        };
        match (rows.get(&function), args.get(2)) {
            (Some(Some(row)), _) => Ok(value(row)),
            (_, Some(placeholder @ cel::Value::Map(_))) => Ok(placeholder.clone()),
            _ => Err(cel::ExecutionError::function_error("measured", "nothing measured relates to this machine")),
        }
    });
    context
}

fn real(held: &cel::Value) -> Option<f64> {
    match held {
        cel::Value::Int(n) => Some(*n as f64),
        cel::Value::UInt(n) => Some(*n as f64),
        cel::Value::Float(n) => Some(*n),
        _ => None,
    }
}

/// `expression`'s number in `context`; why not, in words.
fn number(expression: &str, context: &cel::Context) -> Result<f64, String> {
    let program = compiled(expression)?;
    let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| program.execute(context)));
    match answer {
        Ok(Ok(held)) => match real(&held) {
            Some(n) if n.is_finite() => Ok(n),
            Some(_) => Err("it gives no finite number".into()),
            None => Err(format!("it gives {}, not a number", named(&held))),
        },
        Ok(Err(error)) => Err(first_line(&error.to_string())),
        Err(_) => Err("it can't be evaluated".into()),
    }
}
