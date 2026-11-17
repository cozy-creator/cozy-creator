//! A lock never waits on a machine that doesn't answer (cl-1064; Paul: "lock must succeed when asked"). While a lock waits on
//! `local`, its status is asked every second, given two to answer; three misses in a row and the lock goes on without it. The
//! project's runs there settle here as canceled, its view is ended, and the machine's clean-up is owed: the view's path (new at each
//! unlock) and the project's folder, in state.db, nothing else of the project. Calcifer pays it when the machine next answers, and
//! before any new work goes to it: everything the machine ran in that view is cancelled, then retired. A slow machine judged wrongly
//! costs nothing but that deferred clean-up: its runs were ended by the lock, and it is told the rest when it answers.
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tracing::{info, warn};

use crate::machines::Machines;

/// How often a lock asks a machine it waits on whether it answers, how long it gives it, and how many misses in a row mean it doesn't.
const ASK_EVERY: Duration = Duration::from_secs(1);
const ASK_WITHIN: Duration = Duration::from_secs(2);
const MISSES: u32 = 3;

/// When each machine last missed `MISSES` asks in a row: one lock reuses that verdict for `VERDICT_KEPT` rather than ask again at
/// each of its steps; anything heard from the machine clears it (`answered_local`).
static UNANSWERED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> = std::sync::LazyLock::new(Default::default);
const VERDICT_KEPT: Duration = Duration::from_secs(15);

fn unanswered_lately(name: &str) -> bool {
    UNANSWERED.lock().expect("unanswered").get(name).is_some_and(|at| at.elapsed() < VERDICT_KEPT)
}

fn verdict(name: &str, answers: bool) {
    let mut unanswered = UNANSWERED.lock().expect("unanswered");
    if answers { unanswered.remove(name); } else { unanswered.insert(name.to_owned(), std::time::Instant::now()); }
}

/// Whether state.db may hold an owed clean-up: true at start, so the first answer looks; false once a look finds none.
static MAYBE_OWED: AtomicBool = AtomicBool::new(true);
/// One payer at a time.
static PAYING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl Machines {
    /// Whether `name` answers its status now: asked up to three times, a second apart, two seconds each.
    pub(crate) async fn answers(&self, name: &str) -> bool {
        if unanswered_lately(name) {
            return false;
        }
        for ask in 0..MISSES {
            if ask > 0 {
                tokio::time::sleep(ASK_EVERY).await;
            }
            if self.asked(name).await {
                verdict(name, true);
                return true;
            }
        }
        verdict(name, false);
        false
    }

    /// Whether `name` answers one status ask now, whatever was judged of it lately (cl-1064): a lock asks before it cuts a save
    /// for a verdict, so a machine that came back keeps its saves. An answer clears the verdict.
    pub(crate) async fn answers_now(&self, name: &str) -> bool {
        let answered = self.asked(name).await;
        if answered {
            verdict(name, true);
        }
        answered
    }

    /// `work`, which waits on `name`, while `name` answers: its status asked every second meanwhile; None once it missed three in
    /// a row, and `work` is dropped.
    pub(crate) async fn while_answering<T>(&self, name: &str, work: impl Future<Output = T>) -> Option<T> {
        if unanswered_lately(name) {
            return None;
        }
        let watch = async {
            let mut misses = 0;
            loop {
                tokio::time::sleep(ASK_EVERY).await;
                misses = if self.asked(name).await { 0 } else { misses + 1 };
                if misses >= MISSES {
                    verdict(name, false);
                    return;
                }
            }
        };
        tokio::select! {
            done = work => Some(done),
            () = watch => None,
        }
    }

    async fn asked(&self, name: &str) -> bool {
        let Ok(Some(link)) = self.calcifer.link(name) else { return false };
        matches!(tokio::time::timeout(ASK_WITHIN, link.status()).await, Ok(Ok(_)))
    }

    /// A lock that couldn't reach `local` owes it the clean-up of the project at `folder`, mounted at `view`.
    pub(crate) fn owe(&self, view: &Path, folder: &Path) -> crate::store::Result<()> {
        self.store().owe("local", view, folder)?;
        MAYBE_OWED.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Whether `local` is owed a clean-up of the project in `folder` (cl-1064): only that project's runs wait for it.
    pub(crate) fn owes(&self, folder: &Path) -> bool {
        self.maybe_owed() && self.store().owed_to("local").is_ok_and(|owed| owed.iter().any(|(_, owed)| owed == folder))
    }

    /// The folders `local` is owed a clean-up of (cl-1064): nothing is warmed for them on it until it's paid.
    pub(crate) fn owed_folders(&self) -> Vec<std::path::PathBuf> {
        if !self.maybe_owed() {
            return vec![];
        }
        self.store().owed_to("local").map(|owed| owed.into_iter().map(|(_, folder)| folder).collect()).unwrap_or_default()
    }

    /// Whether `local` may still be owed a clean-up (cheap: no read once one found none).
    pub(crate) fn maybe_owed(&self) -> bool {
        MAYBE_OWED.load(Ordering::Relaxed)
    }

    /// Pays what locks owe `local`, now that it answers: each view's runs cancelled and its records retired. True once nothing is
    /// owed; false when it didn't take it (not answering, or refusing), and it's asked again when it next answers.
    pub(crate) async fn pay_owed(self: &Arc<Self>) -> bool {
        if !self.maybe_owed() {
            return true;
        }
        let _paying = PAYING.lock().await;
        let owed: Vec<(PathBuf, PathBuf)> = match self.store().owed_to("local") {
            Ok(owed) => owed,
            Err(e) => {
                warn!("the clean-ups locks owe local could not be read: {e}");
                return false;
            }
        };
        if owed.is_empty() {
            MAYBE_OWED.store(false, Ordering::Relaxed);
            return true;
        }
        let Ok(Some(link)) = self.calcifer.link("local") else { return false };
        for (view, folder) in owed {
            match tokio::time::timeout(ASK_WITHIN * 5, link.project_gone_at(&view)).await {
                Ok(Ok(())) => {
                    if let Err(e) = self.store().paid("local", &view) {
                        warn!(folder = %folder.display(), "local did a locked project's clean-up, not yet recorded: {e}");
                        return false;
                    }
                    // Only now its sealed view goes: nothing local ran there is left to write into it.
                    if let Err(e) = crate::project::vault::retire_owed_view(&folder, &view) {
                        warn!(folder = %folder.display(), "a locked project's ended view stays in its folder: {}", e.code);
                    }
                    info!(folder = %folder.display(), "local did the clean-up a lock owed it for this project");
                }
                Ok(Err(e)) => {
                    let code = match &e {
                        crate::link::LinkError::Refused { code, .. } => code.as_str(),
                        _ => "machine_unreachable",
                    };
                    warn!(folder = %folder.display(), code, "local didn't take the clean-up a lock owes it for this project; asked again when it next answers");
                    return false;
                }
                Err(_) => {
                    warn!(folder = %folder.display(), "local didn't finish the clean-up a lock owes it for this project in time; asked again when it next answers");
                    return false;
                }
            }
        }
        // What waited for it is warmed now (`Lists::send` holds an owed project's lists back from local).
        self.lists.again(self);
        true
    }

    /// `local` answered: what locks owe it is paid now, before anything else is sent to it.
    pub(crate) fn answered_local(&self) {
        verdict("local", true);
        if !self.maybe_owed() {
            return;
        }
        if let Some(machines) = self.calcifer.machines() {
            tokio::spawn(async move {
                machines.pay_owed().await;
            });
        }
    }
}
