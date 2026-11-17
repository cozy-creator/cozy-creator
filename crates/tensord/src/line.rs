//! The machine's install line (cl-905): one environment built at a time, in job order, beside its
//! download line (TensorFS's queue, `origin::priority`) and its GPU queue, so the next job installs
//! while this one downloads or runs. A lone job never waits. A caller's guess (`origin::priority::prefetch`) holding the turn
//! gives it up to a run's install that comes for it, and builds again after it (cl-1104).
use std::{
    collections::BTreeMap,
    sync::{Condvar, LazyLock, Mutex, MutexGuard, PoisonError},
};

use crate::stop::Stop;

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
    /// Its order, and what a run's install cancels when the holder is a caller's guess (`Turn::outranked`).
    held: Option<(u64, Stop)>,
    /// Who waits, by order and then arrival.
    waiting: BTreeMap<(u64, u64), String>,
    arrivals: u64,
}

/// A turn in the line, given up when it drops: its line, whose it is, what a run's install cancels to take it from a guess,
/// and whether its run held the turn already.
pub(crate) struct Turn(&'static Line, String, Stop, bool);

impl Turn {
    /// Canceled once a run's install came for the line while a caller's guess held it: the guess's build stops, gives its turn
    /// up and goes again after that run's.
    pub(crate) fn outranked(&self) -> &Stop {
        &self.2
    }

    /// Its run held the turn already: the turn is given up where it was first taken.
    pub(crate) fn nested(&self) -> bool {
        self.3
    }
}

impl Line {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run `caller`'s turn to do `what`, at `order` (smaller first, then arrival). While it waits,
    /// `said` hears its place and whose turn it is, and None once its turn comes. A run holding the
    /// turn already has it at once. None: `stop` ended the wait.
    pub(crate) fn turn(&'static self, order: u64, caller: &str, what: &str, stop: Option<&Stop>, said: &dyn Fn(Option<String>)) -> Option<Turn> {
        let mut state = self.state();
        if state.holder.as_mut().filter(|(holder, ..)| holder == caller).map(|(_, _, held)| *held += 1).is_some() {
            let outranked = state.held.as_ref().map(|(_, outranked)| outranked.clone()).unwrap_or_default();
            return Some(Turn(self, caller.to_owned(), outranked, true));
        }
        state.arrivals += 1;
        let key = (order, state.arrivals);
        state.waiting.insert(key, caller.to_owned());
        // A caller's guess building now gives way to a run's install: told with the line let go, as its build's steps hear it.
        let guess = |order: u64| crate::origin::priority::prefetch(order);
        if let Some(outranked) = state.held.as_ref().filter(|(held, _)| guess(*held) && !guess(order)).map(|(_, outranked)| outranked.clone()) {
            drop(state);
            outranked.cancel();
            state = self.state();
        }
        let mut shown: Option<String> = None;
        // A turn given up or the stop wakes the wait.
        let _woken = stop.map(|stop| stop.on_cancel(move || {
            drop(self.state());
            self.moved.notify_all();
        }));
        loop {
            if state.holder.is_none() && state.waiting.keys().next() == Some(&key) {
                state.waiting.remove(&key);
                state.holder = Some((caller.to_owned(), what.to_owned(), 1));
                let outranked = Stop::default();
                state.held = Some((order, outranked.clone()));
                drop(state);
                if shown.is_some() {
                    said(None);
                }
                tracing::info!(run = caller, what, order, "install line: its turn");
                return Some(Turn(self, caller.to_owned(), outranked, false));
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
                Some((holder, doing, _)) => format!("{place} in line to install, after {holder} installs {doing}"),
                None => format!("{place} in line to install"),
            };
            if shown.as_ref() != Some(&text) {
                drop(state);
                tracing::info!(run = caller, "install line: {text}");
                said(Some(text.clone()));
                shown = Some(text);
                state = self.state();
                continue;
            }
            state = self.moved.wait(state).unwrap_or_else(PoisonError::into_inner);
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
                state.held = None;
                tracing::info!(run = self.1, "install line: done");
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
