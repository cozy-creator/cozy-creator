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
pub(crate) const MAX_STDERR: u64 = 64 << 10;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct TriageRef {
    pub subject_id: String,
    /// Hex SHA-256 of the bundle's canonical bytes.
    pub sha256: String,
    pub length: u64,
}

/// What the machine knows of one failed attempt.
pub(crate) struct Facts<'a> {
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

/// `text` within `limit` chars: whole, or its head and its tail around a marker. A failure's
/// first line names the operation and its last lines the cause (uv's error after its progress).
pub(crate) fn ends(text: &str, limit: usize) -> String {
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
pub(crate) fn masked(text: &str) -> String {
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

/// What travels with a failed run (`Reason.detail`, and a failed child's answer to its caller): its
/// raising traceback, else the end of its executor's stderr with repeats collapsed, secrets masked.
pub(crate) fn detail(bundle: &[u8]) -> Option<String> {
    const TRACEBACK_LINES: usize = 30;
    const STDERR_LINES: usize = 12;
    const DETAIL_BYTES: usize = 8 << 10;
    let bundle: serde_json::Value = serde_json::from_slice(bundle).ok()?;
    let text = |field: &serde_json::Value| field.as_str().map(str::trim).filter(|text| !text.is_empty()).map(str::to_owned);
    let (lines, most) = match text(&bundle["terminal"]["traceback"]) {
        Some(traceback) => (traceback.lines().map(str::to_owned).collect(), TRACEBACK_LINES),
        None => (collapsed(&text(&bundle["executor"]["stderr_tail"])?), STDERR_LINES),
    };
    let lines: Vec<String> = lines;
    let tail = lines[lines.len().saturating_sub(most)..].join("\n");
    Some(masked(capped(&tail, DETAIL_BYTES)))
}

/// `text`'s lines, each once where it last came, with how many times, its digits aside: a CUDA
/// assert writes one line per GPU thread, then all of it again.
fn collapsed(text: &str) -> Vec<String> {
    let mut lines: Vec<(&str, u32)> = vec![];
    let mut seen = std::collections::HashMap::new();
    for line in text.lines().rev() {
        let key: String = line.chars().filter(|c| !c.is_ascii_digit()).collect();
        let at = *seen.entry(key).or_insert_with(|| {
            lines.push((line, 0));
            lines.len() - 1
        });
        lines[at].1 += 1;
    }
    lines.into_iter().rev().map(|(line, times)| match times {
        1 => line.to_owned(),
        times => format!("{line} (x{times})"),
    }).collect()
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
pub(crate) fn write(
    root: &Path,
    facts: &Facts,
    executor: Option<&crate::journal::ExecutorFacts>,
) -> io::Result<TriageRef> {
    let subject_id = format!("trb-{}", &crate::uuid::simple()[..24]);
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
pub(crate) fn read(root: &Path, triage: &TriageRef) -> io::Result<Vec<u8>> {
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
