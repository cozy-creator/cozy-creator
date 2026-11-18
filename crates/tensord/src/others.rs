//! Other runs, as a run's words say them to its readers (cl-1217, cl-1223): a project's runs and local packages are its own, so a
//! reader outside it hears how many and of what kind, never which. Every wait and refusal that names other runs says them through
//! `Others`.
use api::machine::LocalPrivateStorage;

use crate::{execution::Engine, journal::Execution};

/// How far a run's readers read another run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Read {
    /// Not at all: another sender's, or another project's.
    No,
    /// By its id: its sender's in no project, or on a rental, which knows no projects (its client says another project's run as
    /// only that, `Calcifer::runs_named_for`). Not the package it installs, which may be another project's.
    Id,
    /// In their own project: by its id, and the package it installs.
    Project,
}

/// How far the readers of a run `sender` sent in `project` (none: in no project) read run `id`, a caller's id: what
/// `Engine::get_public` finds of `sender`'s in that project, then in none.
pub(crate) fn read_by(engine: &Engine, sender: &str, project: Option<&LocalPrivateStorage>, id: &str) -> Read {
    let sent = |scope| crate::private_io::within(scope, || engine.get_public(Some(sender), id).is_ok());
    if project.is_some_and(|project| sent(Some(project.clone()))) {
        Read::Project
    } else if sent(None) {
        Read::Id
    } else {
        Read::No
    }
}

/// `read_by` from both runs' records, which hold what the journal finds them by: their sender and project.
pub(crate) fn read_between(reader: &Execution, other: &Execution) -> Read {
    fn sender(run: &Execution) -> Option<&str> {
        run.submission.as_ref().map(|submission| submission.actor.as_str())
    }
    fn project(run: &Execution) -> Option<&str> {
        run.invocation.local_private.as_ref().map(|scope| scope.project_id.as_str())
    }
    if sender(reader).is_none() || sender(reader) != sender(other) {
        return Read::No;
    }
    match (project(reader), project(other)) {
        (_, None) => Read::Id,
        (Some(mine), Some(theirs)) if mine == theirs => Read::Project,
        _ => Read::No,
    }
}

/// Other runs, by their callers' ids, as readers `reads` admits hear them: those they read by id, any other counted as another
/// project's, and a list's item never (`ahead::hidden`).
pub(crate) struct Others {
    named: Vec<String>,
    unnamed: usize,
    listed: usize,
    /// How far they read the one they read most, a list's item too.
    pub(crate) read: Read,
}

impl Others {
    pub(crate) fn of<I: AsRef<str>>(runs: impl IntoIterator<Item = I>, reads: impl Fn(&str) -> Read) -> Self {
        let mut others = Self { named: vec![], unnamed: 0, listed: 0, read: Read::No };
        for run in runs {
            let run = run.as_ref();
            let read = reads(run);
            others.read = others.read.max(read);
            match (crate::ahead::hidden(run), read) {
                (true, _) => others.listed += 1,
                (false, Read::No) => others.unnamed += 1,
                (false, _) => others.named.push(run.to_owned()),
            }
        }
        others
    }

    /// None at all, lists' items too.
    pub(crate) fn none(&self) -> bool {
        self.named.is_empty() && self.unnamed == 0 && self.listed == 0
    }

    /// The first named, which its readers can follow.
    pub(crate) fn first(&self) -> Option<&str> {
        self.named.first().map(String::as_str)
    }

    /// `run-1b58…, run-77c0… and another project's run`, the last joined by `and`; None: only lists' items, or none.
    pub(crate) fn said(&self, and: &str) -> Option<String> {
        let mut said = self.named.clone();
        match self.unnamed {
            0 => {}
            1 => said.push("another project's run".to_owned()),
            n => said.push(format!("{n} other projects' runs")),
        }
        match said.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            [rest @ .., last] => Some(format!("{} {and} {last}", rest.join(", "))),
        }
    }
}
