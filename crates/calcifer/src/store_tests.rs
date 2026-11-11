use super::*;
use std::{sync::{Arc, mpsc}, time::Duration};

fn store() -> Arc<Store> {
    Arc::new(Store {
        writer: Mutex::new(Connection::open_in_memory().unwrap()),
        reader: Mutex::new(Connection::open_in_memory().unwrap()),
    })
}

/// Both async workers contend for the same store lock while an unrelated task must run.
/// The lock is released externally, so even a failing implementation shuts down cleanly.
fn assert_lock_wait_yields(writer: bool) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let store = store();
    let (locked, lock_taken) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let locked_store = store.clone();
    let holder = std::thread::spawn(move || {
        let lock = if writer { &locked_store.writer } else { &locked_store.reader };
        let _held = lock.lock().unwrap();
        locked.send(()).unwrap();
        let _ = released.recv();
    });
    lock_taken.recv().unwrap();
    let (entering, entered) = mpsc::channel();
    let mut waiting = Vec::new();
    for _ in 0..2 {
        let store = store.clone();
        let entering = entering.clone();
        waiting.push(runtime.spawn(async move {
            entering.send(()).unwrap();
            if writer {
                store.write(|_| Ok(())).unwrap();
            } else {
                let value = store.read(|connection| {
                    Ok(connection.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))?)
                }).unwrap();
                assert_eq!(value, 1);
            }
        }));
    }
    drop(entering);
    let both_entered = (0..2).all(|_| entered.recv_timeout(Duration::from_secs(3)).is_ok());
    let (heartbeat, heard) = mpsc::channel();
    let unrelated = runtime.spawn(async move { heartbeat.send(()).unwrap(); });
    let responsive = heard.recv_timeout(Duration::from_secs(1)).is_ok();
    release.send(()).unwrap();
    holder.join().unwrap();
    runtime.block_on(async {
        for wait in waiting { wait.await.unwrap(); }
        unrelated.await.unwrap();
    });
    assert!(both_entered, "both store operations must reach the held lock");
    assert!(responsive, "store lock waits blocked both async workers; an unrelated request could not run");
}

#[test]
fn reader_lock_wait_keeps_async_tasks_runnable() {
    assert_lock_wait_yields(false);
}

#[test]
fn writer_lock_wait_keeps_async_tasks_runnable() {
    assert_lock_wait_yields(true);
}

#[test]
fn reads_work_on_current_thread_runtime_and_without_runtime() {
    let store = store();
    assert_eq!(store.read(|_| Ok(7)).unwrap(), 7);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    assert_eq!(runtime.block_on(async { store.read(|_| Ok(9)) }).unwrap(), 9);
}
