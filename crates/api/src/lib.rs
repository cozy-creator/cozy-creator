//! The types of every cozy API, and nothing that speaks one. `calcifer` is Calcifer's HTTP API, which the CLI and
//! the page call; `machine` is a machine's (TensorD's), which Calcifer calls; the rest are the pieces and formats
//! they share. No HTTP, TLS or async runtime: each client brings its own. The exceptions are the `hub` feature, the Hub's
//! client, which Calcifer and TensorD both speak through, and `update`, cozy's own update, which the CLI and Calcifer share.
#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

pub mod calcifer;
pub mod choices;
pub mod code;
pub mod config;
pub mod contract;
pub mod digests;
pub mod error;
pub mod estimate;
#[cfg(feature = "hub")]
pub mod hub;
pub mod interface;
pub mod ids;
pub mod machine;
pub mod model_ref;
pub mod names;
pub mod prerelease;
pub mod release;
pub mod stage;
pub mod target;
pub mod tree;
#[cfg(feature = "update")]
pub mod update;
pub mod yaml;

/// The commit this build is from (`build.rs`).
pub const COMMIT: &str = env!("COZY_COMMIT");
/// This cozy's version (`build.rs`): a release (`0.25.0`), or the open RC's dev version with its commit (`0.26.0-dev+g1a2b3c4d`).
pub const VERSION: &str = env!("COZY_VERSION");
