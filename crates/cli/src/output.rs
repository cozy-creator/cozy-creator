//! Rendering every verb shares. Structured documents use TOON, or JSON with `--json` (NDJSON while
//! following). Views keep their selected fields and bound long text; `--full` retains the whole document.
//! Existing table and record callers retain their rendering while they migrate to structured output.
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
const ALARM: Style = AnsiColor::Red.on_default().bold();

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

/// Print a structured document in the selected output format.
pub(crate) fn document<T: Serialize + ?Sized>(ctx: &Context, value: &T) {
    if ctx.json {
        json(value);
    } else {
        toon(value);
    }
}

fn toon<T: Serialize + ?Sized>(value: &T) {
    match toon_format::encode_default(&value) {
        Ok(text) => anstream::println!("{text}"),
        Err(_) => { refuse(false, &encoding_error().error); }
    }
}

fn encoding_error() -> Envelope {
    Envelope {
        error: Error::new(api::error::Kind::Internal, "output_encoding", "could not format the command's response")
            .next("cozy --help"),
    }
}

/// Print selected fields with bounded text, or the complete document with `--full`.
pub(crate) fn view<T: Serialize + ?Sized>(ctx: &Context, value: &T, paths: &[&str]) {
    if ctx.full {
        return document(ctx, value);
    }
    let value = match serde_json::to_value(value) {
        Ok(value) => value,
        Err(_) => { refuse(ctx.json, &encoding_error().error); return; }
    };
    let mut value = pick(value, paths);
    truncate(&mut value, None);
    document(ctx, &value);
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

/// Bound display text recursively while retaining identifiers and usable locations.
fn truncate(value: &mut serde_json::Value, field: Option<&str>) {
    use serde_json::Value;
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                truncate(value, Some(name));
            }
        }
        Value::Array(items) => {
            for value in items {
                truncate(value, field);
            }
        }
        Value::String(text) if !field.is_some_and(reference_field) => {
            let count = text.chars().count();
            if count > TEXT {
                let preview: String = text.chars().take(TEXT).collect();
                *text = format!("{preview}... (truncated, {count} chars total; use --full)");
            }
        }
        _ => {}
    }
}

fn reference_field(name: &str) -> bool {
    let name = name.rsplit('_').next().unwrap_or(name);
    matches!(name,
        "id" | "ids" | "path" | "paths" | "url" | "urls" | "uri" | "uris" |
        "file" | "files" | "folder" | "directory" | "root" | "sha256" | "hash" |
        "digest" | "key" | "name" | "target" | "command" | "next" | "help"
    )
}

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

/// Prints a human refusal on stderr, or a JSON envelope on stdout, and answers its exit code.
pub(crate) fn refusal(ctx: &Context, error: &Error) -> ExitCode {
    refuse(ctx.json, error)
}

pub(crate) fn refuse(json: bool, error: &Error) -> ExitCode {
    if json {
        self::json(&Envelope { error: error.clone() });
    } else {
        anstream::eprint!("{}", human_error(error));
    }
    ExitCode::from(error.exit_code() as u8)
}

fn human_error(error: &Error) -> String {
    let mut text = String::new();
    error_line(&mut text, &format!("error[{}]: ", error.code), &error.message);
    if let Some(cause) = &error.cause {
        if cause != &error.code {
            error_line(&mut text, "cause: ", cause);
        }
    }
    if let Some(seconds) = error.retry_after_s {
        error_line(&mut text, "hint: ", &format!("try again after {seconds} seconds"));
    }
    if let Some(remedy) = &error.remedy {
        let command = remedy.trim().starts_with("cozy ") && remedy.trim().lines().count() == 1;
        error_line(&mut text, if command { "next: " } else { "hint: " }, remedy);
    }
    for next in &error.next {
        if error.remedy.as_deref().map(str::trim) != Some(next.trim()) {
            error_line(&mut text, "next: ", next);
        }
    }
    text
}

fn error_line(out: &mut String, prefix: &str, value: &str) {
    for (index, line) in shown(value).trim().lines().enumerate() {
        out.push_str(if index == 0 { prefix } else { "  " });
        out.push_str(line);
        out.push('\n');
    }
}

pub(crate) fn failure(ctx: &Context, message: &str, next: &[String]) {
    if ctx.json {
        return;
    }
    let mut text = String::new();
    error_line(&mut text, "", message);
    for next in next {
        error_line(&mut text, "next: ", next);
    }
    anstream::eprint!("{text}");
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

/// `key  value` lines, keys aligned, then notes. An alarm's value is red.
#[derive(Default)]
pub(crate) struct Record {
    fields: Vec<(String, String, bool)>,
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
        self.fields.push((key.to_owned(), shown(&value.to_string()), false));
        self
    }

    /// A field that says something is wrong now (a machine that makes its runs slow), red.
    pub(crate) fn alarm(mut self, key: &str, value: impl Display) -> Self {
        self.fields.push((key.to_owned(), shown(&value.to_string()), true));
        self
    }

    pub(crate) fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(shown(&note.into()));
        self
    }

    pub(crate) fn print(self, _ctx: &Context) {
        for line in self.styled(KEY, DIM, ALARM) {
            anstream::println!("{line}");
        }
    }

    /// The lines `print` writes, unstyled: keys aligned, then the notes.
    pub(crate) fn lines(&self) -> Vec<String> {
        self.styled(Style::new(), Style::new(), Style::new())
    }

    /// `lines`, each with whether it is an alarm.
    pub(crate) fn marked(&self) -> Vec<(String, bool)> {
        let alarms = self.fields.iter().map(|(.., alarm)| *alarm).chain(self.notes.iter().map(|_| false));
        self.lines().into_iter().zip(alarms).collect()
    }

    fn styled(&self, key: Style, note: Style, alarm: Style) -> Vec<String> {
        let width = self.fields.iter().map(|(k, ..)| k.len()).max().unwrap_or(0);
        let fields = (self.fields.iter()).map(|(k, v, alarmed)| match alarmed {
            true => format!("{key}{k:<width$}{key:#}  {alarm}{v}{alarm:#}"),
            false => format!("{key}{k:<width$}{key:#}  {v}"),
        });
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

#[cfg(test)]
mod tests {
    use super::{TEXT, human_error, pick, truncate};
    use api::error::{Envelope, Error};
    use serde_json::{Value, json};

    #[test]
    fn truncated_text_retains_unicode_count_and_full_hint() {
        let mut value = json!({ "runs": [{ "input": { "prompt": "猫".repeat(TEXT + 17) } }] });
        truncate(&mut value, None);
        let text = value["runs"][0]["input"]["prompt"].as_str().unwrap();
        assert!(text.starts_with(&"猫".repeat(TEXT)));
        assert!(text.ends_with("... (truncated, 817 chars total; use --full)"));
    }

    #[test]
    fn short_text_and_references_stay_whole() {
        let value = json!({
            "description": "x".repeat(TEXT),
            "run_id": "x".repeat(TEXT + 1),
            "path": format!("/{}", "x".repeat(TEXT + 1)),
            "urls": [format!("https://example.com/{}", "x".repeat(TEXT + 1))],
            "next": ["x".repeat(TEXT + 1)],
        });
        let mut truncated = value.clone();
        truncate(&mut truncated, None);
        assert_eq!(truncated, value);
    }

    #[test]
    fn projected_data_round_trips_through_toon() {
        let value = json!({ "runs": [
            { "id": "7", "state": "completed", "message": "a,b\n\"quoted\"", "hidden": 1 },
            { "id": "8", "state": "running", "message": "ready", "hidden": 2 },
        ], "hidden": true });
        let selected = pick(value, &["runs.id", "runs.state", "runs.message"]);
        let encoded = toon_format::encode_default(&selected).unwrap();
        let decoded: Value = toon_format::decode_default(&encoded).unwrap();
        assert_eq!(decoded, selected);
        assert!(encoded.starts_with("runs[2]{id,state,message}:"));
    }

    #[test]
    fn runtime_errors_render_multiline_remedies_without_escaping() {
        let mut error = Error::unavailable("machine_offline", "The machine is offline.\nConnection refused.")
            .remedy("Start its service.\nThen retry this request.")
            .next("cozy machine ls")
            .next("cozy doctor");
        error.cause = Some("connection_refused".into());
        error.retry_after_s = Some(5);
        let text = human_error(&error);
        assert_eq!(text, "error[machine_offline]: The machine is offline.\n  Connection refused.\ncause: connection_refused\nhint: try again after 5 seconds\nhint: Start its service.\n  Then retry this request.\nnext: cozy machine ls\nnext: cozy doctor\n");
        assert!(!text.contains("\\n"));
        assert_eq!(error.exit_code(), 1);
    }

    #[test]
    fn help_remedies_are_commands_and_duplicate_next_lines_are_omitted() {
        let error = Error::new(api::error::Kind::Usage, "usage", "unrecognized subcommand 'ls'")
            .remedy("cozy --help")
            .next("cozy --help");
        assert_eq!(human_error(&error), "error[usage]: unrecognized subcommand 'ls'\nnext: cozy --help\n");
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn structured_errors_keep_multiline_remedies_and_backend_details() {
        let mut error = Error::unavailable("machine_offline", "The machine is offline.\nConnection refused.")
            .remedy("Start its service.\nThen retry this request.")
            .next("cozy machine ls");
        error.cause = Some("connection_refused".into());
        error.retry_after_s = Some(5);
        error.metadata = Some(json!({"machine": "test", "detail": "first\nsecond"}));
        let envelope = Envelope { error: error.clone() };
        let serialized = serde_json::to_string(&envelope).unwrap();
        let restored: Envelope = serde_json::from_str(&serialized).unwrap();
        assert_eq!(restored.error, error);
        assert!(serialized.contains("\\n"));
        assert!(!human_error(&error).contains("metadata"));
    }
}
