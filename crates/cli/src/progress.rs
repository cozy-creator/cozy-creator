//! Status lines on stderr. `Line` can be hidden; `Transfer` uses one TTY line or sparse
//! non-TTY snapshots, including with --json, without writing progress to stdout.
use std::{
    io::Write as _,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

/// The spinner's frames; a finished line shows a blank.
const SPIN: [char; 29] = [
    '⠁', '⠁', '⠉', '⠙', '⠚', '⠒', '⠂', '⠂', '⠒', '⠲', '⠴', '⠤', '⠄', '⠄', '⠤', '⠠', '⠠', '⠤', '⠦', '⠖', '⠒', '⠐', '⠐',
    '⠒', '⠓', '⠋', '⠉', '⠈', '⠈',
];
/// Updates closer together than this draw once.
const PACE: Duration = Duration::from_millis(50);
/// Where the line's bar goes: it takes the width the rest leaves.
pub(crate) const WIDE_BAR: char = '\0';

/// What a line's layout reads.
pub(crate) struct Shown<'a> {
    state: &'a State,
}

impl Shown<'_> {
    pub(crate) fn spinner(&self) -> char {
        match self.state.finished {
            true => ' ',
            false => SPIN[(self.state.tick % SPIN.len() as u64) as usize],
        }
    }

    pub(crate) fn msg(&self) -> &str {
        &self.state.msg
    }

    /// Bytes moved, `12.50 MiB`.
    pub(crate) fn bytes(&self) -> Bytes {
        Bytes(self.state.pos)
    }

    pub(crate) fn total_bytes(&self) -> Bytes {
        Bytes(self.state.len.unwrap_or(0))
    }

    /// The recent rate, `12.50 MiB/s`.
    pub(crate) fn bytes_per_sec(&self) -> String {
        format!("{}/s", Bytes(self.state.rate as u64))
    }

    /// Time since the line began, in its largest unit: `5s`, `2m`, `3h`.
    pub(crate) fn elapsed(&self) -> String {
        elapsed(self.state.started.elapsed())
    }
}

/// `n` bytes in binary units, two decimals past 1 KiB.
pub(crate) struct Bytes(pub u64);

impl std::fmt::Display for Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const UNITS: [&str; 8] = ["Ki", "Mi", "Gi", "Ti", "Pi", "Ei", "Zi", "Yi"];
        let mut value = self.0 as f64;
        let mut unit = 0;
        while value >= 1024.0 && unit < UNITS.len() {
            value /= 1024.0;
            unit += 1;
        }
        match unit {
            0 => write!(f, "{value:.0} B"),
            _ => write!(f, "{value:.2} {}B", UNITS[unit - 1]),
        }
    }
}

fn elapsed(took: Duration) -> String {
    const UNITS: [(u64, &str); 6] = [(31_536_000, "y"), (604_800, "w"), (86_400, "d"), (3600, "h"), (60, "m"), (1, "s")];
    let took = took.as_secs_f64();
    // The largest unit it is at least one and a half of, shown rounded and at least 2.
    let (index, (unit, name)) = UNITS
        .iter()
        .copied()
        .enumerate()
        .find(|&(i, (unit, _))| match UNITS.get(i + 1) {
            Some(&(next, _)) => took + next as f64 / 2.0 >= unit as f64 * 1.5,
            None => true,
        })
        .expect("seconds match");
    let mut n = (took / unit as f64).round() as u64;
    if index < UNITS.len() - 1 {
        n = n.max(2);
    }
    format!("{n}{name}")
}

struct State {
    msg: String,
    pos: u64,
    len: Option<u64>,
    tick: u64,
    started: Instant,
    /// Bytes a second, smoothed, and the position and instant it was last measured at.
    rate: f64,
    measured: (u64, Instant),
    drawn: Option<Instant>,
    /// A thread turns the spinner, not updates.
    steady: bool,
    finished: bool,
}

struct Shared {
    state: Mutex<State>,
    layout: fn(&Shown) -> String,
}

/// A line dropped unfinished stays, complete, and what follows prints below it.
impl Drop for Shared {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        if !state.finished {
            state.pos = state.len.unwrap_or(state.pos);
            state.finished = true;
            draw(self.layout, state, true, None);
        }
    }
}

/// A status line; clones draw the same one.
#[derive(Clone)]
pub(crate) struct Line(Option<Arc<Shared>>);

impl Line {
    /// A line on stderr when `shown`, laid out by `layout` ([`WIDE_BAR`] marks the bar), else hidden.
    pub(crate) fn stderr(shown: bool, len: Option<u64>, layout: fn(&Shown) -> String) -> Self {
        let now = Instant::now();
        let state = State {
            msg: String::new(),
            pos: 0,
            len,
            tick: 0,
            started: now,
            rate: 0.0,
            measured: (0, now),
            drawn: None,
            steady: false,
            finished: false,
        };
        Self(shown.then(|| Arc::new(Shared { state: Mutex::new(state), layout })))
    }

    /// Redraws every `every` from a thread of its own, the spinner turning, until finished.
    pub(crate) fn steady(&self, every: Duration) {
        let Some(shared) = &self.0 else { return };
        shared.state.lock().unwrap_or_else(|e| e.into_inner()).steady = true;
        let weak = Arc::downgrade(shared);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(every);
                let Some(shared) = weak.upgrade() else { return };
                let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.finished {
                    return;
                }
                state.tick += 1;
                draw(shared.layout, &mut state, true, None);
            }
        });
    }

    pub(crate) fn set_message(&self, msg: impl Into<String>) {
        self.update(|state| state.msg = msg.into(), true);
    }

    pub(crate) fn set_length(&self, len: u64) {
        self.update(|state| state.len = Some(len), true);
    }

    pub(crate) fn set_position(&self, pos: u64) {
        self.update(|state| state.pos = pos, false);
    }

    pub(crate) fn inc(&self, delta: u64) {
        self.update(|state| state.pos = state.pos.saturating_add(delta), false);
    }

    /// `text` on a line of its own above the status line.
    pub(crate) fn println(&self, text: impl AsRef<str>) {
        let Some(shared) = &self.0 else { return };
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let finished = state.finished;
        draw(shared.layout, &mut state, !finished, Some(text.as_ref()));
    }

    /// Clears the line; nothing draws it again.
    pub(crate) fn finish_and_clear(&self) {
        let Some(shared) = &self.0 else { return };
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.finished = true;
        let mut err = std::io::stderr().lock();
        let _ = write!(err, "\r\x1b[2K");
        let _ = err.flush();
    }

    fn update(&self, change: impl FnOnce(&mut State), always: bool) {
        let Some(shared) = &self.0 else { return };
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.finished {
            return;
        }
        change(&mut state);
        let now = Instant::now();
        let (at, since) = state.measured;
        let seconds = now.duration_since(since).as_secs_f64();
        if seconds >= 0.25 {
            // About the last 3 s of moving; the first measure as it is.
            let recent = state.pos.saturating_sub(at) as f64 / seconds;
            let weight = if state.rate == 0.0 { 1.0 } else { (seconds / 3.0).min(1.0) };
            state.rate += (recent - state.rate) * weight;
            state.measured = (state.pos, now);
        }
        let paced = state.drawn.is_none_or(|drawn| now.duration_since(drawn) >= PACE);
        if paced && !state.steady {
            state.tick += 1;
        }
        draw(shared.layout, &mut state, always || paced, None);
    }
}

/// Logical completion and observed I/O are separate: cached/resumed bytes must not inflate throughput.
#[derive(Clone, Copy, Default)]
pub(crate) struct TransferCounts {
    pub(crate) files_done: u64,
    pub(crate) files_total: Option<u64>,
    pub(crate) bytes_done: u64,
    pub(crate) bytes_total: Option<u64>,
    /// Cumulative bytes actually moved by this caller, excluding its cached baseline.
    pub(crate) bytes_moved: u64,
}

struct TransferState {
    phase: String,
    finalizing: bool,
    counts: TransferCounts,
    measured: Option<(u64, Instant)>,
    rate: Option<f64>,
    logged: Option<Instant>,
    finished: bool,
}

struct TransferShared {
    label: String,
    tty: bool,
    line: Line,
    state: Mutex<TransferState>,
}

impl Drop for TransferShared {
    fn drop(&mut self) {
        // Line's legacy drop fills its bar. A transfer that failed/interrupted must only clear it.
        if !self.state.get_mut().unwrap_or_else(|e| e.into_inner()).finished { self.line.finish_and_clear(); }
    }
}

/// A cloneable transfer meter. `tty=false` logs sparse stderr snapshots; it does not hide progress.
#[derive(Clone)]
pub(crate) struct Transfer(Arc<TransferShared>);

impl Transfer {
    pub(crate) fn stderr(tty: bool, label: impl Into<String>) -> Self {
        let label: String = label.into();
        let line = Line::stderr(tty, None, |shown| format!("{} {}", shown.spinner(), shown.msg()));
        if tty { line.steady(Duration::from_millis(100)); }
        Self(Arc::new(TransferShared { label: single_line(&label), tty, line, state: Mutex::new(TransferState {
            phase: "starting".into(), finalizing: false, counts: TransferCounts::default(),
            measured: None, rate: None, logged: None, finished: false,
        }) }))
    }

    pub(crate) fn phase(&self, phase: &str) { self.set_phase(phase, false); }

    /// Payload completion is not operation completion: verification/commit can continue without a transfer ETA/bar.
    pub(crate) fn finalizing(&self, phase: &str) { self.set_phase(phase, true); }

    fn set_phase(&self, phase: &str, finalizing: bool) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.finished { return; }
        let phase = single_line(phase);
        let changed = state.phase != phase || state.finalizing != finalizing;
        let mode_changed = state.finalizing != finalizing;
        state.phase = phase;
        state.finalizing = finalizing;
        self.0.show(&mut state, changed && (finalizing || mode_changed));
    }

    /// The first snapshot establishes a rate baseline. Later snapshots may change totals as inventory expands.
    pub(crate) fn update(&self, counts: TransferCounts) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.finished { return; }
        let known = (state.counts.files_total.is_none() && counts.files_total.is_some())
            || (state.counts.bytes_total.is_none() && counts.bytes_total.is_some());
        let now = Instant::now();
        match state.measured {
            None => state.measured = Some((counts.bytes_moved, now)),
            Some((at, _)) if counts.bytes_moved < at => {
                state.measured = Some((counts.bytes_moved, now));
                state.rate = None;
            }
            Some((at, since)) => {
                let seconds = now.duration_since(since).as_secs_f64();
                if seconds >= 0.25 {
                    let recent = (counts.bytes_moved - at) as f64 / seconds;
                    let before = state.rate.unwrap_or(0.0);
                    let weight = if before == 0.0 { 1.0 } else { (seconds / 3.0).min(1.0) };
                    state.rate = Some(before + (recent - before) * weight);
                    state.measured = Some((counts.bytes_moved, now));
                }
            }
        }
        state.counts = counts;
        self.0.show(&mut state, known);
    }

    /// A warning belongs above the active meter, never in its counters or final JSON.
    pub(crate) fn println(&self, text: impl AsRef<str>) {
        let state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let text = single_line(text.as_ref());
        if self.0.tty && !state.finished { self.0.line.println(text); }
        else { let _ = writeln!(std::io::stderr().lock(), "{text}"); }
    }

    /// Only the caller's terminal success/failure ends the meter. Reaching a payload total does not.
    pub(crate) fn finish_and_clear(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.finished { return; }
        state.finished = true;
        self.0.line.finish_and_clear();
    }
}

impl TransferShared {
    fn show(&self, state: &mut TransferState, force: bool) {
        if self.tty {
            let width = crossterm::terminal::size().map_or(80, |(columns, _)| columns as usize);
            let message = transfer_message(&self.label, state, Some(width.saturating_sub(2)));
            self.line.update(|line| {
                line.msg = message;
                line.pos = state.counts.bytes_done;
                line.len = state.counts.bytes_total;
            }, force);
        } else if force || state.logged.is_none_or(|at| at.elapsed() >= Duration::from_secs(5)) {
            let _ = writeln!(std::io::stderr().lock(), "{}", transfer_message(&self.label, state, None));
            state.logged = Some(Instant::now());
        }
    }
}

fn transfer_message(label: &str, state: &TransferState, width: Option<usize>) -> String {
    let counts = state.counts;
    let files = counts.files_total.map_or_else(|| "?".into(), |total| total.to_string());
    let total = counts.bytes_total.map_or_else(|| "?".into(), |total| format!("{:.1}", total as f64 / 1_000_000.0));
    let informed = counts.files_total.is_some() || counts.bytes_total.is_some() || counts.files_done > 0 || counts.bytes_done > 0;
    let mut details = if informed { format!("{}/{files} files {:.1}/{total} MB", counts.files_done, counts.bytes_done as f64 / 1_000_000.0) } else { String::new() };
    let bar = width.is_some() && counts.bytes_total.is_some() && !state.finalizing;
    if !state.finalizing && informed {
        let speed = state.rate.map_or_else(|| "—".into(), |rate| format!("{:.1}", rate / 1_000_000.0));
        let eta = counts.bytes_total.zip(state.rate).and_then(|(total, rate)| {
            (total > counts.bytes_done && rate > 0.0).then(|| (total - counts.bytes_done) as f64 / rate)
        }).and_then(|seconds| Duration::try_from_secs_f64(seconds).ok()).map(elapsed);
        let eta = eta.map_or_else(|| "—".into(), |eta| format!("~{eta}"));
        details.push_str(&format!(" {}{speed} MB/s ETA {eta}", if bar { format!("{WIDE_BAR} ") } else { String::new() }));
    }
    let phase = state.phase.replace('_', " ");
    let mut prefix = format!("{label}: {phase}");
    if let Some(width) = width {
        // Keep counts/rate/ETA intact. Filenames can be long hashes; spend only the
        // remaining columns on the phase/label, leaving a small bar where possible.
        let budget = width.saturating_sub(details.width() + 1 + if bar { 8 } else { 0 });
        if prefix.width() > budget {
            if budget >= 6 {
                let phase_width = phase.width().min(budget - 6);
                prefix = format!("{}: {}", bounded(label, budget - phase_width - 2), bounded(&phase, phase_width));
            } else { prefix = bounded(&phase, budget); }
        }
    }
    if prefix.is_empty() { details } else if details.is_empty() { prefix } else { format!("{prefix} {details}") }
}

fn single_line(text: &str) -> String {
    let mut clean = String::new();
    for character in api::names::shown(text).chars() {
        if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') { clean.extend(character.escape_default()); }
        else { clean.push(character); }
    }
    clean
}

fn bounded(text: &str, width: usize) -> String {
    if text.width() <= width { return text.into(); }
    if width == 0 { return String::new(); }
    let mut text = text.to_owned();
    truncate(&mut text, width - 1);
    text.push('…');
    text
}

/// The status line drawn again over itself, `above` printed first. It fills the terminal's
/// width, so whatever prints next starts on the line below.
fn draw(layout: fn(&Shown) -> String, state: &mut State, now: bool, above: Option<&str>) {
    if !now {
        return;
    }
    state.drawn = Some(Instant::now());
    let width = crossterm::terminal::size().map_or(80, |(columns, _)| columns as usize);
    let laid = layout(&Shown { state });
    let mut line = match laid.split_once(WIDE_BAR) {
        Some((before, after)) => {
            let left = width.saturating_sub(before.width() + after.width());
            format!("{before}{}{after}", bar(fraction(state), left))
        }
        None => laid,
    };
    let used = truncate(&mut line, width);
    let mut err = std::io::stderr().lock();
    let _ = write!(err, "\r\x1b[2K");
    if let Some(above) = above {
        let _ = writeln!(err, "{above}");
    }
    let _ = write!(err, "{line}{:pad$}", "", pad = width - used);
    let _ = err.flush();
}

fn fraction(state: &State) -> f64 {
    match (state.pos, state.len) {
        (_, None) => 0.0,
        (_, Some(0)) => 1.0,
        (pos, Some(len)) => (pos as f64 / len as f64).clamp(0.0, 1.0),
    }
}

/// Filled cells, the leading one, and empty cells, `width` in all.
fn bar(fraction: f64, width: usize) -> String {
    let fill = fraction * width as f64;
    let full = fill as usize;
    let lead = fill > 0.0 && full < width;
    format!("{}{}{}", "━".repeat(full), if lead { "╸" } else { "" }, " ".repeat(width - full - lead as usize))
}

/// `line` cut to `width` columns, so it never wraps; the columns it takes.
fn truncate(line: &mut String, width: usize) -> usize {
    let mut used = 0;
    for (at, c) in line.char_indices() {
        let next = used + c.width().unwrap_or(0);
        if next > width {
            line.truncate(at);
            break;
        }
        used = next;
    }
    used
}

#[cfg(test)]
mod transfer_tests {
    use super::*;
    use unicode_width::UnicodeWidthStr as _;

    fn sample() -> TransferState {
        TransferState {
            phase: "copying".into(), finalizing: false,
            counts: TransferCounts { files_done: 1, files_total: Some(2), bytes_done: 1_000_000,
                bytes_total: Some(2_000_000), bytes_moved: 1_000_000 },
            measured: None, rate: Some(500_000.0), logged: None, finished: false,
        }
    }

    #[test]
    fn decimal_megabytes_and_estimated_eta() {
        let shown = transfer_message("vault encrypt", &sample(), None);
        assert!(shown.contains("1/2 files 1.0/2.0 MB"));
        assert!(shown.contains("0.5 MB/s ETA ~2s"));
        assert!(!shown.contains("MiB") && !shown.contains("1000000"));
    }

    #[test]
    fn unknown_totals_and_finished_payload_do_not_invent_an_eta() {
        let mut state = sample();
        state.counts.files_total = None;
        state.counts.bytes_total = None;
        let unknown = transfer_message("download", &state, Some(78));
        assert!(unknown.contains("1/? files 1.0/? MB") && unknown.contains("ETA —"));
        assert!(!unknown.contains(WIDE_BAR));
        state.counts.bytes_total = Some(state.counts.bytes_done);
        assert!(transfer_message("download", &state, None).contains("ETA —"));
    }

    #[test]
    fn finalization_keeps_counts_without_claiming_transfer_completion() {
        let mut state = sample();
        state.phase = "compacting_history".into();
        state.finalizing = true;
        let shown = transfer_message("vault encrypt", &state, Some(78));
        assert!(shown.contains("compacting history") && shown.contains("1/2 files 1.0/2.0 MB"));
        assert!(!shown.contains("ETA") && !shown.contains("MB/s") && !shown.contains(WIDE_BAR));
    }

    #[test]
    fn long_file_labels_leave_numeric_tail_visible_at_eighty_columns() {
        let mut state = sample();
        state.phase = format!("Sending file {}.bin", "a".repeat(64));
        let shown = transfer_message("Downloading outputs", &state, Some(78));
        assert!(shown.width() <= 78);
        assert!(shown.contains("1/2 files 1.0/2.0 MB") && shown.contains("0.5 MB/s ETA ~2s"));
        assert!(!shown.contains(&"a".repeat(64)));
    }

    #[test]
    fn cached_baseline_and_phase_changes_do_not_become_network_rate() {
        let meter = Transfer::stderr(false, "Sending files");
        meter.update(TransferCounts { files_done: 5, files_total: Some(10), bytes_done: 8_000_000_000,
            bytes_total: Some(16_000_000_000), bytes_moved: 0 });
        meter.phase("copying");
        meter.finalizing("verifying");
        let state = meter.0.state.lock().unwrap();
        assert!(state.rate.is_none());
        assert_eq!(state.measured.unwrap().0, 0);
        assert_eq!(state.counts.bytes_done, 8_000_000_000);
    }

    #[test]
    fn labels_are_one_line_without_double_escaping_literal_text() {
        assert_eq!(single_line("a\nb\r\t\u{1b}"), "a\\nb\\r\\t\\u{1b}");
        assert_eq!(single_line(r"already\nescaped"), r"already\nescaped");
    }
}
