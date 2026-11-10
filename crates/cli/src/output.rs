//! Rendering every verb shares. With `--json` stdout carries one JSON document (NDJSON while
//! following), compact unless stdout is a terminal, holding what the view shows under the record's own
//! names (`--full`: all of it). Otherwise tables and `key  value` records, root names without `_/`: a view
//! is one short block on stdout, warnings included, its long text cut and said so (`--full`: whole).
//! Progress and prompts go to stderr. Colors go through anstream, which drops them for pipes and `NO_COLOR`.
use std::{
    fmt::Display,
    io::{IsTerminal, Write as _},
    path::Path,
    process::ExitCode,
};

use anstyle::{AnsiColor, Style};
use comfy_table::{ContentArrangement, presets};
use api::error::{Envelope, Error};
use api::names::shown;
use serde::Serialize;

use crate::Context;

/// The exit of a follower that heard its run stop moving (Calcifer's word, after 10 minutes with no progress): the run goes on.
pub(crate) const STALLED: u8 = 4;

const KEY: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();
const WARN: Style = AnsiColor::Yellow.on_default().bold();
const FAIL: Style = AnsiColor::Red.on_default().bold();

/// What the terminal can show, read once at the entry.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Term {
    /// stdout is a terminal: tables fit its width and paths become links.
    pub stdout: bool,
    /// stderr is a terminal: live progress may redraw there.
    pub stderr: bool,
    hyperlinks: bool,
}

impl Term {
    pub(crate) fn detect() -> Self {
        let stdout = std::io::stdout().is_terminal();
        Self {
            stdout,
            stderr: std::io::stderr().is_terminal(),
            hyperlinks: stdout && hyperlinks(),
        }
    }
}

/// Whether the terminal shows OSC 8 links: `FORCE_HYPERLINK` (anything but `0`), or one known to.
fn hyperlinks() -> bool {
    let var = |name| std::env::var(name).ok();
    if let Some(force) = var("FORCE_HYPERLINK") {
        return force.trim() != "0";
    }
    let vte = var("VTE_VERSION").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0) >= 5000;
    let program = matches!(
        var("TERM_PROGRAM").as_deref(),
        Some("Hyper" | "iTerm.app" | "terminology" | "WezTerm" | "vscode" | "ghostty" | "zed")
    );
    let term = matches!(var("TERM").as_deref(), Some("xterm-kitty" | "alacritty" | "alacritty-direct"));
    let colorterm = var("COLORTERM").as_deref() == Some("xfce4-terminal");
    ["DOMTERM", "WT_SESSION", "KONSOLE_VERSION"].iter().any(|name| var(name).is_some()) || vte || program || term || colorterm
}

/// Prints one JSON document on stdout: indented for a terminal, one line for anything else.
pub(crate) fn json<T: Serialize + ?Sized>(document: &T) {
    let text = match std::io::stdout().is_terminal() {
        true => serde_json::to_string_pretty(document),
        false => serde_json::to_string(document),
    };
    anstream::println!("{}", text.expect("a document serializes"));
}

/// Prints `document` as `--json` does for a view: only the fields `paths` name, or all with `--full`.
pub(crate) fn view<T: Serialize + ?Sized>(ctx: &Context, document: &T, paths: &[&str]) {
    match ctx.full {
        true => json(document),
        false => json(&pick(serde_json::to_value(document).expect("a document serializes"), paths)),
    }
}

/// `value` holding only what `paths` name (`machines.rental.sku`): a list is walked through, and a
/// path that ends at a field keeps it whole.
fn pick(value: serde_json::Value, paths: &[&str]) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Array(items) => items.into_iter().map(|item| pick(item, paths)).collect(),
        Value::Object(fields) => fields
            .into_iter()
            .filter_map(|(key, field)| {
                let under: Vec<&str> = paths
                    .iter()
                    .filter_map(|path| path.strip_prefix(key.as_str()))
                    .filter_map(|rest| match rest {
                        "" => Some(""),
                        rest => rest.strip_prefix('.'),
                    })
                    .collect();
                match under.as_slice() {
                    [] => None,
                    _ if under.contains(&"") => Some((key, field)),
                    _ => Some((key, pick(field, &under))),
                }
            })
            .collect(),
        other => other,
    }
}

/// The most of a text a view shows; `--full` shows it whole.
const TEXT: usize = 800;

/// `text` as a view shows it: past a quarter more than `TEXT` characters, its first `TEXT`, or its last
/// when `tail` (a traceback ends with what failed), saying how long it is.
pub(crate) fn cut(ctx: &Context, text: &str, tail: bool) -> String {
    let count = text.chars().count();
    if ctx.full || count <= TEXT + TEXT / 4 {
        return text.to_owned();
    }
    let said = format!("(cut, {count} chars: --full)");
    match tail {
        true => format!("{said} ...{}", text.chars().skip(count - TEXT).collect::<String>()),
        false => format!("{}... {said}", text.chars().take(TEXT).collect::<String>()),
    }
}

/// Prints one NDJSON line on stdout, flushed: a follower's reader sees it at once.
pub(crate) fn ndjson<T: Serialize + ?Sized>(event: &T) {
    let mut out = anstream::stdout().lock();
    let line = serde_json::to_string(event).expect("an event serializes");
    let _ = writeln!(out, "{line}").and_then(|()| out.flush());
}

/// Prints a refusal on stdout (the envelope with `--json`) and answers its exit code.
pub(crate) fn refusal(ctx: &Context, error: &Error) -> ExitCode {
    refuse(ctx.json, error)
}

pub(crate) fn refuse(json: bool, error: &Error) -> ExitCode {
    if json {
        self::json(&Envelope {
            error: error.clone(),
        });
    } else {
        anstream::println!("{FAIL}error{FAIL:#}: {} ({})", shown(&error.message), error.code);
        if let Some(remedy) = &error.remedy {
            anstream::println!("  {}", shown(remedy));
        }
        for next in &error.next {
            anstream::println!("  {DIM}next:{DIM:#} {}", shown(next));
        }
    }
    ExitCode::from(error.exit_code() as u8)
}

pub(crate) fn failure(ctx: &Context, message: &str, next: &[String]) {
    if ctx.json {
        return;
    }
    anstream::println!("{}", shown(message));
    for next in next {
        anstream::println!("  {DIM}next:{DIM:#} {}", shown(next));
    }
}

/// What the command is doing, for a person on stderr; silent with `--json`.
pub(crate) fn say(ctx: &Context, message: &str) {
    if !ctx.json {
        anstream::eprintln!("{}", shown(message));
    }
}

/// A warning on stderr: the command goes on.
pub(crate) fn warn(message: &str) {
    anstream::eprintln!("{WARN}warning{WARN:#}: {}", shown(message));
}

/// A line that is a warning, said as written.
pub(crate) fn warned(line: &str) {
    anstream::eprintln!("{}", shown(line));
}

/// Whether `name` holds every one of `words`, in any case: `ls` words over what a machine holds.
pub(crate) fn names(name: &str, words: &[String]) -> bool {
    let name = name.to_lowercase();
    words.iter().all(|word| name.contains(&word.to_lowercase()))
}

/// `value` held between 1 and `most`, said when that moved it.
pub(crate) fn at_most(flag: &str, value: u32, most: u32) -> u32 {
    let held = value.clamp(1, most);
    if held != value {
        warn(&format!("{flag} {value}: using {held}"));
    }
    held
}

/// At most two commands to try next, for a person; a JSON document carries its own.
pub(crate) fn next(ctx: &Context, commands: &[&str]) {
    if ctx.json {
        return;
    }
    for command in commands.iter().take(2) {
        anstream::println!("{DIM}next:{DIM:#} {}", shown(command));
    }
}

/// The commands Calcifer's answer names, every one.
pub(crate) fn nexts(ctx: &Context, commands: &[String]) {
    for command in commands {
        next(ctx, &[command]);
    }
}

/// A local path, as an OSC 8 link on a terminal that opens them; the bare path anywhere else.
pub(crate) fn link(ctx: &Context, path: &Path) -> String {
    link_on(ctx.term, path)
}

/// `path` as `term` can show it: a link where it can.
pub(crate) fn link_on(term: Term, path: &Path) -> String {
    let shown = path.display().to_string();
    hyperlink(term, path, &shown).unwrap_or(shown)
}

/// `shown` as an OSC 8 link to the absolute `path`, on a terminal that opens them.
pub(crate) fn hyperlink(term: Term, path: &Path, shown: &str) -> Option<String> {
    if !term.hyperlinks || !path.is_absolute() {
        return None;
    }
    let url = url::Url::from_file_path(path).ok()?;
    Some(format!("\x1b]8;;{url}\x1b\\{shown}\x1b]8;;\x1b\\"))
}

/// `key  value` lines, keys aligned, then notes.
#[derive(Default)]
pub(crate) struct Record {
    fields: Vec<(String, String)>,
    notes: Vec<String>,
}

impl Record {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// One line per top-level key of `value`'s JSON: the same view `--json` prints whole.
    pub(crate) fn of<T: Serialize + ?Sized>(value: &T) -> Self {
        let mut record = Self::new();
        if let Ok(serde_json::Value::Object(map)) = serde_json::to_value(value) {
            for (key, value) in map {
                record = record.field(&key, plain(&value));
            }
        }
        record
    }

    pub(crate) fn field(mut self, key: &str, value: impl Display) -> Self {
        self.fields.push((key.to_owned(), shown(&value.to_string())));
        self
    }

    pub(crate) fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(shown(&note.into()));
        self
    }

    pub(crate) fn print(self, _ctx: &Context) {
        for line in self.styled(KEY, DIM) {
            anstream::println!("{line}");
        }
    }

    /// The lines `print` writes, unstyled: keys aligned, then the notes.
    pub(crate) fn lines(&self) -> Vec<String> {
        self.styled(Style::new(), Style::new())
    }

    fn styled(&self, key: Style, note: Style) -> Vec<String> {
        let width = self.fields.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        let fields = (self.fields.iter()).map(|(k, v)| format!("{key}{k:<width$}{key:#}  {v}"));
        let notes = self.notes.iter().map(|n| format!("{note}{n}{note:#}"));
        fields.chain(notes).collect()
    }
}

/// A JSON value as a person reads it: strings bare, lists comma-joined, absent as `-`.
fn plain(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "-".into(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items.iter().map(plain).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// A byte count in binary units: `512 B`, `6.9 GiB`.
pub(crate) fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    match unit {
        0 => format!("{n} B"),
        _ if value < 10.0 => format!("{value:.1} {}", UNITS[unit]),
        _ => format!("{value:.0} {}", UNITS[unit]),
    }
}

/// How long ago a Unix-milliseconds instant was, in its two largest units: `42s`, `3m12s`,
/// `5h3m`, `2d4h`.
pub(crate) fn age(at_ms: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    span((now - at_ms).max(0) / 1000)
}

/// `s` seconds in their two largest units: `42s`, `3m12s`, `5h3m`, `2d4h`.
pub(crate) fn span(s: i64) -> String {
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{}s", s / 60, s % 60),
        3600..86400 => format!("{}h{}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d{}h", s / 86400, s % 86400 / 3600),
    }
}

/// Columns without borders. On a terminal rows wrap to its width; elsewhere each row is one
/// whole line, for grep and awk.
pub(crate) struct Table {
    table: comfy_table::Table,
    notes: Vec<String>,
}

impl Table {
    pub(crate) fn new<S: Display>(ctx: &Context, header: impl IntoIterator<Item = S>) -> Self {
        let mut table = comfy_table::Table::new();
        table.load_style(presets::NOTHING);
        if ctx.term.stdout {
            table.set_content_arrangement(ContentArrangement::Dynamic);
        } else {
            table.force_no_tty();
        }
        table.set_header(header.into_iter().map(|h| {
            let cell = comfy_table::Cell::new(h);
            if ctx.term.stdout {
                cell.add_attribute(comfy_table::Attribute::Bold)
            } else {
                cell
            }
        }));
        Self {
            table,
            notes: vec![],
        }
    }

    pub(crate) fn row<S: Display>(mut self, cells: impl IntoIterator<Item = S>) -> Self {
        self.table.add_row(cells.into_iter().map(|c| shown(&c.to_string())));
        self
    }

    /// A line under the table, such as "12 more: cozy run ls --before 40".
    pub(crate) fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(shown(&note.into()));
        self
    }

    pub(crate) fn print(mut self) {
        for column in self.table.column_iter_mut() {
            column.set_padding((0, 2));
        }
        anstream::println!("{}", self.table.trim_fmt());
        for note in &self.notes {
            anstream::println!("{DIM}{note}{DIM:#}");
        }
    }
}
