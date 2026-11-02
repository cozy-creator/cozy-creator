//! Actual sender/receiver wire round trip for one, two and four native owner intents.
use protocol::executor::Command;
use serde_json::{Value, json};

fn main() {
    let mut frames = Vec::new();
    for degree in [1, 2, 4] {
        let intents: Vec<_> = (0..degree)
            .map(|rank| {
                json!({
                    "uuid": format!("GPU-13800000-0000-0000-0000-{rank:012}"),
                    "directory": "/dev/shm/varena-rank-metadata-only",
                    "shared": false, "priority": rank,
                })
            })
            .collect();
        let frame = json!({
            "cmd": "start", "devices": (0..degree).map(|rank| rank.to_string()).collect::<Vec<_>>().join(","),
            "sequence_parallel_degree": degree, "cooperation": intents,
        });
        let command = Command::decode(&serde_json::to_vec(&frame).unwrap()).unwrap();
        let Command::Start(ref start) = command else {
            panic!("wrong command")
        };
        assert_eq!(start.cooperation.len(), degree);
        for (rank, selected) in start.cooperation.iter().enumerate() {
            assert_eq!(selected.priority, rank as i32);
            assert_eq!(selected.uuid, intents[rank]["uuid"].as_str().unwrap());
        }
        let sent = serde_json::to_value(&command).unwrap();
        assert_eq!(sent["cooperation"], frame["cooperation"]);
        let Command::Start(received) =
            Command::decode(&serde_json::to_vec(&sent).unwrap()).unwrap()
        else {
            panic!("wrong round-trip command")
        };
        assert_eq!(received, *start);
        frames.push(sent);
        for old in [&intents[0], &Value::Null] {
            let invalid = json!({"cmd": "start", "cooperation": old});
            assert!(Command::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
    }
    println!(
        "{}",
        json!({"passed": true, "gpu_qualified": false, "frames": frames})
    );
}
