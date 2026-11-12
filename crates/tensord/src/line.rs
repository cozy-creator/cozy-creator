//! The machine's install line (cl-905): one environment built at a time, in job order, beside its
//! download line (TensorFS's queue, `origin::priority`) and its GPU queue, so the next job installs
//! while this one downloads or runs. A lone job never waits.
use std::{
    collections::BTreeMap,
    sync::{Condvar, LazyLock, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use crate::stop::Stop;

/// How often a wait looks at its stop.
const TICK: Duration = Duration::from_millis(100);

pub(crate) static INSTALLS: LazyLock<Line> = LazyLock::new(Line::default);

#[derive(Default)]
pub(crate) struct Line {
    state: Mutex<State>,
    moved: Condvar,
}

#[derive(Default)]
struct State {
    /// The run whose turn it is (as its caller names it), what it does, and how many turns it holds.
    holder: Option<(String, String, usize)>,
    /// Who waits, by order and then arrival.
    waiting: BTreeMap<(u64, u64), String>,
    arrivals: u64,
}

/// A turn in the line, given up when it drops.
pub(crate) struct Turn(&'static Line);

impl Line {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run `caller`'s turn to do `what`, at `order` (smaller first, then arrival). While it waits,
    /// `said` hears its place and whose turn it is, and None once its turn comes. A run holding the
    /// turn already has it at once. None: `stop` ended the wait.
    pub(crate) fn turn(&'static self, order: u64, caller: &str, what: &str, stop: Option<&Stop>, said: &dyn Fn(Option<String>)) -> Option<Turn> {
        let mut state = self.state();
        if let Some((_, _, held)) = state.holder.as_mut().filter(|(holder, ..)| holder == caller) {
            *held += 1;
            return Some(Turn(self));
        }
        state.arrivals += 1;
        let key = (order, state.arrivals);
        state.waiting.insert(key, caller.to_owned());
        let mut shown: Option<String> = None;
        loop {
            if state.holder.is_none() && state.waiting.keys().next() == Some(&key) {
                state.waiting.remove(&key);
                state.holder = Some((caller.to_owned(), what.to_owned(), 1));
                drop(state);
                if shown.is_some() {
                    said(None);
                }
                return Some(Turn(self));
            }
            if stop.is_some_and(Stop::is_cancelled) {
                state.waiting.remove(&key);
                self.moved.notify_all();
                drop(state);
                if shown.is_some() {
                    said(None);
                }
                return None;
            }
            let place = ordinal(state.waiting.range(..key).count() + 1);
            let text = match &state.holder {
                Some((holder, doing, _)) => format!("{place} in line to install, behind run {holder}'s {doing}"),
                None => format!("{place} in line to install"),
            };
            if shown.as_ref() != Some(&text) {
                drop(state);
                said(Some(text.clone()));
                shown = Some(text);
                state = self.state();
                continue;
            }
            state = self.moved.wait_timeout(state, TICK).unwrap_or_else(PoisonError::into_inner).0;
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut state = self.0.state();
        if let Some((_, _, held)) = state.holder.as_mut() {
            *held -= 1;
            if *held == 0 {
                state.holder = None;
            }
        }
        self.0.moved.notify_all();
    }
}

/// A place in line for people: `next`, `2nd`, `3rd`, `4th`.
fn ordinal(place: usize) -> String {
    let suffix = match (place % 10, place % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    match place {
        1 => "next".into(),
        n => format!("{n}{suffix}"),
    }
}
