//! The types of every cozy API, and nothing that speaks one. `calcifer` is Calcifer's HTTP API, which the CLI and
//! the page call; `machine` is a machine's (TensorD's), which Calcifer calls; the rest are the pieces and formats
//! they share. No HTTP, TLS or async runtime: each client brings its own.
#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

pub mod calcifer;
pub mod choices;
pub mod code;
pub mod config;
pub mod contract;
pub mod digests;
pub mod error;
pub mod interface;
pub mod ids;
pub mod machine;
pub mod model_ref;
pub mod names;
pub mod prerelease;
pub mod stage;
pub mod target;
pub mod tree;
pub mod yaml;

/// The commit this build is from (`build.rs`).
pub const COMMIT: &str = env!("COZY_COMMIT");
