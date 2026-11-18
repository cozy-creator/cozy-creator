//! Every machine's GPU use on the events stream while a page shows it (cl-1191, Paul 11-17: "I want to see how much GPUs are being
//! utilized"): each machine's own reading a second (`Machines::follow_gpus`), followed only while a stream's page says it shows them
//! (`Shown::gpus`), so a machine reads its GPUs only then. Nothing is kept: the page holds the minute its traces draw.
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};

use api::calcifer::StreamEvent;
use futures_util::StreamExt as _;
use tokio::{
    sync::{broadcast, watch},
    task::JoinHandle,
};

use crate::{api::Api, machines::Machines};

/// How many streams' pages show GPU use now.
static WATCHING: LazyLock<watch::Sender<usize>> = LazyLock::new(|| watch::channel(0).0);
/// Each reading as it comes, to the streams that show them. One that falls behind misses some: the next is a second away.
static READINGS: LazyLock<broadcast::Sender<StreamEvent>> = LazyLock::new(|| broadcast::channel(256).0);

/// `count` streams' pages show GPU use now.
pub(crate) fn watching(count: usize) {
    WATCHING.send_if_modified(|now| std::mem::replace(now, count) != count);
}

pub(crate) fn readings() -> broadcast::Receiver<StreamEvent> {
    READINGS.subscribe()
}

/// From Calcifer's start to its stop: every machine followed while a page shows GPU use, none otherwise.
pub(crate) fn start(api: Arc<Api>) {
    tokio::spawn(async move {
        let mut watching = WATCHING.subscribe();
        let mut stop = api.stop_signal();
        loop {
            tokio::select! {
                shown = watching.wait_for(|count| *count > 0) => if shown.is_err() { return },
                _ = stop.wait_for(|stop| *stop) => return,
            }
            follow_all(&api, &mut watching, &mut stop).await;
        }
    });
}

/// Each machine not ended, one task each, until no page shows GPU use; one that comes is followed as it does, and one that ends
/// ends its task.
async fn follow_all(api: &Arc<Api>, watching: &mut watch::Receiver<usize>, stop: &mut watch::Receiver<bool>) {
    let mut changed = api.calcifer.machines_changed();
    let mut following: HashMap<String, JoinHandle<()>> = HashMap::new();
    loop {
        following.retain(|_, task| !task.is_finished());
        for name in api.machines.live_names() {
            if let std::collections::hash_map::Entry::Vacant(free) = following.entry(name) {
                let task = tokio::spawn(follow(api.machines.clone(), free.key().clone()));
                free.insert(task);
            }
        }
        tokio::select! {
            _ = watching.wait_for(|count| *count == 0) => break,
            moved = changed.changed() => if moved.is_err() { break },
            _ = stop.wait_for(|stop| *stop) => break,
        }
    }
    // Dropped, each stream ends, and its machine stops reading.
    for task in following.into_values() {
        task.abort();
    }
}

async fn follow(machines: Arc<Machines>, name: String) {
    let Ok(readings) = machines.follow_gpus(&name) else { return };
    let mut readings = std::pin::pin!(readings);
    while let Some(reading) = readings.next().await {
        let _ = READINGS.send(StreamEvent::MachineGpus { name: name.clone(), reading });
    }
}
