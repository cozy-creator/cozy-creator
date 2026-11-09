pub(crate) mod auth;
mod cozy1;
mod http;
mod machine_runs;
mod machine_status;
mod machine_update;
mod server;

/// The machine API's types and Cozy-Cap live in `api`, shared with Calcifer and the CLI.
pub(crate) use api::{machine::capability, machine as v1};

pub(crate) use server::{booting, serve, Boot, Leaf, MachineIdentity};
