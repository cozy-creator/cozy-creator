//! `needs-answers <interface.json> <function>`, a request on stdin: what the function's `needs` rules say of it, as TensorD
//! answers them for a call (cl-1103): `{"needed": [...], "unneeded": [...], "unanswered": [[name, why], ...]}`. The cpu suite
//! holds each call and on-demand read of a cell to it (e2e/cpu/drive.py).
use std::io::Read as _;

use serde_json::{Value, json};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(function)) = (args.next(), args.next()) else {
        eprintln!("usage: needs-answers <interface.json> <function> < request.json");
        std::process::exit(2);
    };
    let read = |what: &str, text: Result<String, std::io::Error>| -> Value {
        match text.map_err(|error| error.to_string()).and_then(|text| serde_json::from_str(&text).map_err(|error| error.to_string())) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("{what}: {error}");
                std::process::exit(1);
            }
        }
    };
    let document = read(&path, std::fs::read_to_string(&path));
    let mut sent = String::new();
    let request = read("the request", std::io::stdin().read_to_string(&mut sent).map(|_| sent));
    let Some(entry) = api::interface::entrypoint(&document, &function) else {
        eprintln!("{path} has no function {function}");
        std::process::exit(1);
    };
    let answers = needs::answers(entry, &request);
    println!("{}", json!({"needed": answers.needed, "unneeded": answers.unneeded, "unanswered": answers.unanswered}));
}
