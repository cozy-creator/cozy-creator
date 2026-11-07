//! `cozy run ls` as a live view (`crate::tui`), fed by Calcifer's events.
use protocol::{
    calcifer::{Run, StreamEvent},
    error::Error,
};

use super::list;
use crate::{Context, calcifer_client::Watch};

/// The runs `cozy run ls` shows, kept as Calcifer says they change; with `--on` a rental, the
/// runs other clients of this account sent it, read again every few seconds.
pub(crate) struct Runs {
    query: list::Query,
    limit: u64,
    runs: Vec<Run>,
    others: Vec<Run>,
    watch: Watch<StreamEvent>,
}

/// How often a rental's runs other clients sent are read again.
const OTHERS_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

impl Runs {
    /// Subscribes, then lists: a run that changes in between is in the stream. Changes match
    /// `--on` by the machine's name, which an alias or rental id resolved to.
    pub(crate) async fn open(ctx: &Context, query: list::Query, limit: u64) -> Result<Self, Error> {
        let watch = ctx.calcifer().await?.changes().await?.quiet();
        let list = list::runs(ctx, &query).await?;
        Ok(Self {
            query: list::Query {
                machine: list.machine.or(query.machine),
                ..query
            },
            limit,
            runs: list.runs.into_iter().filter(list::shown).collect(),
            others: list.others,
            watch,
        })
    }

    pub(crate) async fn changed(&mut self, ctx: &Context) -> Result<(), Error> {
        loop {
            let change = tokio::select! {
                change = self.watch.change() => change,
                () = tokio::time::sleep(OTHERS_EVERY), if !self.others.is_empty() => {
                    self.others = list::runs(ctx, &self.query).await?.others;
                    return Ok(());
                }
            };
            match change {
                Some(StreamEvent::RunUpdated { run }) => {
                    if self.apply(*run) {
                        return Ok(());
                    }
                }
                Some(_) => {}
                // A stream that ended (Calcifer restarted, or this view fell behind) may have
                // missed a change: subscribe and list again.
                None => {
                    *self = Self::open(ctx, self.query.clone(), self.limit).await?;
                    return Ok(());
                }
            }
        }
    }

    /// Takes in `run` as it is now; true when the view changed.
    fn apply(&mut self, run: Run) -> bool {
        let fits = list::matches(&self.query, &run);
        match self.runs.iter().position(|r| r.id == run.id) {
            Some(i) if fits => self.runs[i] = run,
            Some(i) => drop(self.runs.remove(i)),
            None if !fits => return false,
            None => {
                let at = self.runs.partition_point(|r| r.number > run.number);
                self.runs.insert(at, run);
                if self.limit > 0 {
                    self.runs.truncate(self.limit as usize);
                }
            }
        }
        true
    }

    pub(crate) fn title(&self) -> String {
        let title = match self.limit {
            0 => "runs".into(),
            limit => format!("runs, newest {limit}"),
        };
        title + &self.watch.silence().map(|s| format!(" · {s}")).unwrap_or_default()
    }

    pub(crate) fn headers(&self) -> Vec<&'static str> {
        list::HEADERS.to_vec()
    }

    pub(crate) fn rows(&self) -> Vec<Vec<String>> {
        list::rows(&[self.runs.as_slice(), &self.others].concat())
    }

    pub(crate) fn id(&self, selected: usize) -> Option<String> {
        self.runs.iter().chain(&self.others).nth(selected).map(|r| r.id.to_string())
    }
}
