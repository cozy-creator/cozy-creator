pub mod auth;
pub mod backend;
pub mod domain;
mod cozy1;
mod forward;
mod http;
pub(crate) mod machine_runs;
mod machine_status;
mod machine_update;
mod server;

/// The machine API's types and Cozy-Cap live in `api`, shared with Calcifer and the CLI.
pub use api::{machine::capability, machine as v1};

pub use backend::MachineBackend;
pub(crate) use http::refused;
pub use machine_status::CAPABILITIES;
pub use server::{booting, serve, Boot, MachineIdentity};
