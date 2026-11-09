pub(crate) mod auth;
pub(crate) mod backend;
pub(crate) mod domain;
mod cozy1;
mod http;
pub(crate) mod machine_runs;
mod machine_status;
mod machine_update;
mod server;

/// The machine API's types and Cozy-Cap live in `api`, shared with Calcifer and the CLI.
pub(crate) use api::{machine::capability, machine as v1};

pub(crate) use backend::MachineBackend;
pub(crate) use http::refused;
pub(crate) use machine_status::CAPABILITIES;
pub(crate) use server::{booting, serve, Boot, MachineIdentity};
