//! The wire types every cozy process shares.
#![forbid(unsafe_code)]

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
