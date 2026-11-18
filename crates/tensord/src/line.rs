//! The machine's install line (cl-905): one environment built at a time, in job order, beside its
//! download line (TensorFS's queue, `origin::priority`) and its GPU queue, so the next job installs
//! while this one downloads or runs. A lone job never waits. A caller's guess (`origin::priority::prefetch`) holding the turn
//! loses it to a run's install at once, never waits for the guess to let go: the guess's build is told, stops at its next step and
//! builds again after the run's (cl-1104). A run waiting on what a guess holds (the environment it builds) raises that guess to the
//! run's place (`raise`): a run never waits behind guesses (cl-1123: a run waited 2.5 s behind an Orbit guess's short steps).
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Condvar, LazyLock, Mutex, MutexGuard, PoisonError},
};

use crate::{
    others::{Others, Read},
    stop::Stop,
};

pub(crate) static INSTALLS: LazyLock<Line> = LazyLock::new(Line::default);

#[derive(Default)]
pub(crate) struct Line {
    state: Mutex<State>,
    moved: Condvar,
}

#[derive(Default)]
struct State {
    holder: Option<Holder>,
    /// Who waits, by order and then arrival.
    waiting: BTreeMap<(u64, u64), String>,
    arrivals: u64,
    /// The orders runs waiting on a caller raised it to (`raise`), by caller.
    raised: HashMap<String, Vec<u64>>,
}

/// The run whose turn it is.
struct Holder {
    /// As its caller names it, and what it does.
    caller: String,
    what: String,
    /// How many turns it holds, and which taking of the line they are (`Turn::taken`).
    held: usize,
    taken: u64,
    order: u64,
    /// What a run's install cancels as it takes the line from a caller's guess (`Turn::outranked`).
    outranked: Stop,
}

fn guess(order: u64) -> bool {
    crate::origin::priority::prefetch(order)
}

impl State {
    /// The guess holding the line, taken from it for a run's install at `order`: it is canceled once the lock is let go.
    fn outranked_by(&mut self, order: u64) -> Option<Holder> {
        self.holder.take_if(|holder| guess(holder.order) && !guess(order))
    }

    fn order_of(&self, caller: &str, order: u64) -> u64 {
        self.raised.get(caller).into_iter().flatten().fold(order, |order, raised| order.min(*raised))
    }
}

/// A guess's build told it lost the line to a run's install.
fn outranked(holder: Option<Holder>, by: &str) {
    if let Some(holder) = holder {
        tracing::info!(run = holder.caller, "install line: its build of {} gives the line to {by}'s install", holder.what);
        holder.outranked.cancel();
    }
}

/// A turn in the line, given up when it drops, unless a run's install took the line from it before.
pub(crate) struct Turn {
    line: &'static Line,
    caller: String,
    outranked: Stop,
    nested: bool,
    taken: u64,
}

impl Turn {
    /// Canceled once a run's install took the line from this caller's guess: the guess's build stops, and goes again after that
    /// run's.
    pub(crate) fn outranked(&self) -> &Stop {
        &self.outranked
    }

    /// Its run held the turn already: the turn is given up where it was first taken.
    pub(crate) fn nested(&self) -> bool {
        self.nested
    }
}

/// A run's raise of a caller (`Line::raise`), until the run has what it waited on.
pub(crate) struct Raised {
    line: &'static Line,
    caller: String,
    order: u64,
}

impl Line {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run `caller`'s turn to do `what`, at `order` (smaller first, then arrival). While it waits,
    /// `said` hears its place and whose turn it is as its readers may (`reads`, `behind`), and None once its turn comes. A run
    /// holding the turn already has it at once. None: `stop` ended the wait.
    pub(crate) fn turn(
        &'static self,
        order: u64,
        caller: &str,
        what: &str,
        stop: Option<&Stop>,
        said: &dyn Fn(Option<String>),
        reads: &dyn Fn(&str) -> Read,
    ) -> Option<Turn> {
        let mut state = self.state();
        if let Some(holder) = state.holder.as_mut().filter(|holder| holder.caller == caller) {
            holder.held += 1;
            let (outranked, taken) = (holder.outranked.clone(), holder.taken);
            return Some(Turn { line: self, caller: caller.to_owned(), outranked, nested: true, taken });
        }
        let order = state.order_of(caller, order);
        state.arrivals += 1;
        let arrival = state.arrivals;
        state.waiting.insert((order, arrival), caller.to_owned());
        if let Some(holder) = state.outranked_by(order) {
            drop(state);
            outranked(Some(holder), caller);
            self.moved.notify_all();
            state = self.state();
        }
        let mut shown: Option<String> = None;
        // A turn given up, a raise or the stop wakes the wait.
        let _woken = stop.map(|stop| stop.on_cancel(move || {
            drop(self.state());
            self.moved.notify_all();
        }));
        loop {
            // Its place, which a raise moves (`raise`).
            let key = *state.waiting.keys().find(|(_, at)| *at == arrival).expect("its place in line");
            if state.holder.is_none() && state.waiting.keys().next() == Some(&key) {
                state.waiting.remove(&key);
                let outranked = Stop::default();
                state.holder = Some(Holder { caller: caller.to_owned(), what: what.to_owned(), held: 1, taken: arrival, order: key.0, outranked: outranked.clone() });
                drop(state);
                if shown.is_some() {
                    said(None);
                }
                tracing::info!(run = caller, what, order = key.0, "install line: its turn");
                return Some(Turn { line: self, caller: caller.to_owned(), outranked, nested: false, taken: arrival });
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
            let holder = state.holder.as_ref().map(|holder| (holder.caller.clone(), holder.what.clone()));
            let text = match &holder {
                Some((holder, what)) => format!("{place} in line to install, after {holder} installs {what}"),
                None => format!("{place} in line to install"),
            };
            if shown.as_ref() != Some(&text) {
                drop(state);
                tracing::info!(run = caller, "install line: {text}");
                said(Some(behind(&place, holder.as_ref(), reads)));
                shown = Some(text);
                state = self.state();
                continue;
            }
            state = self.moved.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// A run at `order` waits on what `caller` holds (the environment it builds, its package's install): `caller` goes at the run's
    /// place in line until the run has it, so a guess holding it never keeps the run behind other guesses, nor loses the line to
    /// other runs. Nothing for a guess's wait.
    pub(crate) fn raise(&'static self, caller: &str, order: u64) -> Option<Raised> {
        if guess(order) {
            return None;
        }
        let mut state = self.state();
        state.raised.entry(caller.to_owned()).or_default().push(order);
        let mut outranked_holder = None;
        let waiting = state.waiting.iter().find(|(_, waiter)| *waiter == caller).map(|(key, _)| *key);
        if let Some(holder) = state.holder.as_mut().filter(|holder| holder.caller == caller) {
            holder.order = holder.order.min(order);
        } else if let Some(key) = waiting.filter(|(was, _)| order < *was) {
            state.waiting.remove(&key);
            state.waiting.insert((order, key.1), caller.to_owned());
            outranked_holder = state.outranked_by(order);
        }
        drop(state);
        tracing::info!(run = caller, order, "install line: raised for a run that waits on it");
        outranked(outranked_holder, caller);
        self.moved.notify_all();
        Some(Raised { line: self, caller: caller.to_owned(), order })
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut state = self.line.state();
        // A turn a run's install took the line from was given up then.
        if let Some(holder) = state.holder.as_mut().filter(|holder| holder.taken == self.taken) {
            holder.held -= 1;
            if holder.held == 0 {
                state.holder = None;
                tracing::info!(run = self.caller, "install line: done");
            }
        }
        self.line.moved.notify_all();
    }
}

impl Drop for Raised {
    fn drop(&mut self) {
        let mut state = self.line.state();
        let orders = state.raised.entry(self.caller.clone()).or_default();
        if let Some(at) = orders.iter().position(|order| *order == self.order) {
            orders.swap_remove(at);
        }
        if orders.is_empty() {
            state.raised.remove(&self.caller);
        }
    }
}

/// A waiting run's place behind `holder` (its caller's id, and the package it installs) as the run's readers hear it (cl-1223): their
/// own project's run and its package, a run they read by id alone without it, another project's install only as that.
fn behind(place: &str, holder: Option<&(String, String)>, reads: &dyn Fn(&str) -> Read) -> String {
    let Some((holder, what)) = holder else { return format!("{place} in line to install") };
    let whose = Others::of([holder], reads);
    match (whose.read, whose.said("and")) {
        (Read::No, _) => format!("{place} in line to install, after another project's install"),
        (Read::Project, Some(run)) => format!("{place} in line to install, after {run} installs {what}"),
        (Read::Id, Some(run)) => format!("{place} in line to install, after {run} installs its package"),
        // Its project's list item: a guess, which a run's install takes the line from.
        (_, None) => format!("{place} in line to install"),
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
