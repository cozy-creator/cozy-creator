//! The process-tree probe (`e2e/cli/stale_parent.py`): `process-tree kill <pid>` ends `pid` and every process
//! `platform::process::tree` counts as its, as cozy's own callers do (a view let go, an executor ended), and prints their pids.
fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [verb, pid] = arguments.as_slice() else {
        eprintln!("usage: process-tree kill <pid>");
        std::process::exit(2);
    };
    let (true, Ok(pid)) = (verb == "kill", pid.parse::<u32>()) else {
        eprintln!("usage: process-tree kill <pid>");
        std::process::exit(2);
    };
    let tree = platform::process::tree(pid);
    let pids: Vec<String> = tree.iter().map(|process| process.birth().pid.to_string()).collect();
    let ended = platform::process::end_tree(&tree).is_ok();
    println!("{{\"tree\":[{}],\"ended\":{ended}}}", pids.join(","));
}
