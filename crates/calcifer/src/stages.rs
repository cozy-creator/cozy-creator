//! A walk's quote, timed stage by stage (xs-199): each read of a job's package and machine, for the project's walk log, and what is
//! going when a quote runs long. Outside a quote, a stage is just its work.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The stages a quote went through, and those going.
#[derive(Default)]
pub(crate) struct Stages {
    done: Vec<(&'static str, u128)>,
    going: Vec<(&'static str, Instant)>,
}

tokio::task_local! {
    static STAGES: Arc<Mutex<Stages>>;
}

/// `work`, timed as stage `name` of the quote it is part of.
pub(crate) async fn stage<T>(name: &'static str, work: impl Future<Output = T>) -> T {
    let Ok(stages) = STAGES.try_with(Arc::clone) else { return work.await };
    let began = Instant::now();
    stages.lock().expect("stages lock").going.push((name, began));
    let out = work.await;
    let mut held = stages.lock().expect("stages lock");
    held.going.retain(|(going, at)| !(*going == name && *at == began));
    held.done.push((name, began.elapsed().as_millis()));
    out
}

/// `work`, its stages kept in `stages`.
pub(crate) async fn recorded<T>(stages: Arc<Mutex<Stages>>, work: impl Future<Output = T>) -> T {
    STAGES.scope(stages, work).await
}

impl Stages {
    /// `package 12 ms, machine status 15003 ms`, then what is going and for how long.
    pub(crate) fn said(&self) -> String {
        let done = self.done.iter().map(|(name, ms)| format!("{name} {ms} ms"));
        let going = self.going.iter().map(|(name, began)| format!("{name} going for {} ms", began.elapsed().as_millis()));
        let said: Vec<String> = done.chain(going).collect();
        if said.is_empty() { "no stage yet".to_owned() } else { said.join(", ") }
    }
}
