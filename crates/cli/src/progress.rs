//! One status line on stderr, redrawn in place while `cozy` waits; lines printed above it. Off a
//! terminal (or with `--json`) it is hidden and draws nothing.
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
