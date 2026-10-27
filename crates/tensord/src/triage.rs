//! One bounded triage bundle per failed attempt (the Runtime worker's `triage.py`): what a
//! person needs to explain the failure after its executor is gone, as one canonical JSON
//! file. Written and fsynced before the terminal names it, and observation only: nothing in
//! it overrides a terminal field. The CLI quotes `terminal.traceback`.
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};
use platform::durable::Mode;
use tensorfs::sha256;

/// `TriageBundleRef.length` is bounded at 1 MiB by the protocol; fields are capped well below.
const MAX_TRACEBACK: usize = 256 << 10;
const MAX_TEXT: usize = 16 << 10;
/// Enough executor stderr to hold a crash's faulthandler stacks past the extension-module list
/// Python prints after them.
pub const MAX_STDERR: u64 = 64 << 10;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TriageRef {
    pub subject_id: String,
    /// Hex SHA-256 of the bundle's canonical bytes.
    pub sha256: String,
    pub length: u64,
}

/// What the machine knows of one failed attempt.
pub struct Facts<'a> {
    pub request_id: &'a str,
    pub attempt: u32,
    pub terminal: &'a str,
    pub origin: &'a str,
    pub code: &'a str,
    pub message: &'a str,
    pub traceback: &'a str,
    pub executor_pid: u32,
    pub stderr_tail: &'a str,
    /// What ended its GPU group mid-call (`GPU 1's process 5163 ended`), empty otherwise. Every
    /// rank writes the leader's stderr, so the dead rank's own last words are in `stderr_tail`.
    pub fault: &'a str,
}

/// A group fault's evidence: the dead rank, then the stderr all ranks share, its last lines.
const FAULT_LINES: usize = 50;

/// The last `lines` lines of `text` within `bytes`, its end kept.
pub fn tail_lines(text: &str, lines: usize, bytes: usize) -> String {
    let all: Vec<&str> = text.trim_end().lines().collect();
    capped(&all[all.len().saturating_sub(lines)..].join("\n"), bytes).to_string()
}

fn fault_section(bundle: &serde_json::Value, bytes: usize) -> Option<String> {
    let fault = bundle["executor"]["fault"].as_str().filter(|f| !f.is_empty())?;
    let stderr = bundle["executor"]["stderr_tail"].as_str().map(str::trim).unwrap_or_default();
    Some(match stderr.is_empty() {
        true => format!("{fault}; its group's stderr is empty"),
        false => format!("{fault}; the last {FAULT_LINES} lines of its stderr, which every rank writes:\n{}",
            tail_lines(stderr, FAULT_LINES, bytes)),
    })
}

/// `text` within `limit` chars: whole, or its head and its tail around a marker. A failure's
/// first line names the operation and its last lines the cause (uv's error after its progress).
pub fn ends(text: &str, limit: usize) -> String {
    const MARK: &str = " [...] ";
    let count = text.chars().count();
    if count <= limit {
        return text.into();
    }
    let head = limit / 4;
    let tail = limit.saturating_sub(head + MARK.len());
    text.chars().take(head).chain(MARK.chars()).chain(text.chars().skip(count - tail)).collect()
}

/// `scheme://user:secret@host` becomes `scheme://***@host`.
pub fn masked(text: &str) -> String {
    let (mut out, mut rest) = (String::new(), text);
    while let Some(at) = rest.find("://") {
        let (head, after) = rest.split_at(at + 3);
        out.push_str(head);
        let authority = after.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(after.len());
        rest = match after[..authority].rfind('@') {
            Some(user) => {
                out.push_str("***");
                &after[user..]
            }
            None => after,
        };
    }
    out + rest
}

/// What travels with a failed run (its log and `Reason.detail`): its bundle's traceback, else
/// its executor's stderr, the last `DETAIL_LINES` lines within `DETAIL_BYTES`, secrets masked.
pub fn detail(bundle: &[u8]) -> Option<String> {
    const DETAIL_LINES: usize = 40;
    const DETAIL_BYTES: usize = 8 << 10;
    let bundle: serde_json::Value = serde_json::from_slice(bundle).ok()?;
    // A group fault's leader traceback only says the group broke; the dead rank's stderr says why.
    if let Some(section) = fault_section(&bundle, DETAIL_BYTES) {
        return Some(masked(&section));
    }
    let text = [&bundle["terminal"]["traceback"], &bundle["executor"]["stderr_tail"]]
        .into_iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .find(|text| !text.is_empty())?;
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(DETAIL_LINES)..].join("\n");
    Some(masked(capped(&tail, DETAIL_BYTES)))
}

/// A failed attempt's whole raising traceback (else its executor's stderr), secrets masked:
/// what a failed child call hands its caller.
pub fn traceback(bundle: &[u8]) -> Option<String> {
    let bundle: serde_json::Value = serde_json::from_slice(bundle).ok()?;
    let fault = fault_section(&bundle, MAX_STDERR as usize);
    let text = [&bundle["terminal"]["traceback"], &bundle["executor"]["stderr_tail"]]
        .into_iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .find(|text| !text.is_empty());
    match (bundle["terminal"]["traceback"].as_str().map(str::trim).filter(|t| !t.is_empty()), fault) {
        (Some(traceback), Some(fault)) => Some(masked(&format!("{traceback}\n\n{fault}"))),
        (None, Some(fault)) => Some(masked(&fault)),
        _ => text.map(masked),
    }
}

fn capped(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    // Keep the end: a traceback's last lines name the failure.
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Write the bundle under `root/triage/` and return the reference the terminal carries; it
/// names this TensorD and, once an executor said hello, the Runtime and TensorFS it ran.
pub fn write(
    root: &Path,
    facts: &Facts,
    executor: Option<&crate::journal::ExecutorFacts>,
) -> io::Result<TriageRef> {
    let subject_id = format!("trb-{}", &uuid::Uuid::new_v4().simple().to_string()[..24]);
    let mut bundle = json!({
        "format": "cozy.machine.triage/1",
        "subject_id": subject_id,
        "request_id": facts.request_id,
        "attempt_ordinal": facts.attempt,
        "terminal": {
            "terminal": facts.terminal,
            "origin": facts.origin,
            "code": capped(facts.code, MAX_TEXT),
            "message": capped(facts.message, MAX_TEXT),
            "traceback": capped(facts.traceback, MAX_TRACEBACK),
        },
        "executor": {"pid": facts.executor_pid, "stderr_tail": capped(facts.stderr_tail, MAX_STDERR as usize)},
        "software": executor.map(|executor| executor.software(facts.attempt)).unwrap_or_else(|| {
            // A launch failure has no executor; only this TensorD was observed.
            crate::journal::ExecutorFacts::observed(0, String::new(), String::new()).software(facts.attempt)
        }),
    });
    if !facts.fault.is_empty() {
        bundle["executor"]["fault"] = json!(capped(facts.fault, MAX_TEXT));
    }
    let bytes = serde_json_canonicalizer::to_vec(&bundle).map_err(io::Error::other)?;
    let directory = root.join("triage");
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{subject_id}.json"));
    let pending = directory.join(format!(".{subject_id}.pending"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&pending)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&pending, &path)?;
    platform::durable::sync_dir(&directory)?;
    Ok(TriageRef {
        subject_id,
        sha256: sha256::hex(&sha256::digest(&bytes)),
        length: bytes.len() as u64,
    })
}

/// The bundle's bytes, checked against the reference its terminal carries.
pub fn read(root: &Path, triage: &TriageRef) -> io::Result<Vec<u8>> {
    if !triage
        .subject_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "triage subject is not a plain name",
        ));
    }
    let bytes = fs::read(
        root.join("triage")
            .join(format!("{}.json", triage.subject_id)),
    )?;
    if bytes.len() as u64 != triage.length || sha256::hex(&sha256::digest(&bytes)) != triage.sha256
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "triage bundle differs from its terminal's reference",
        ));
    }
    Ok(bytes)
}
