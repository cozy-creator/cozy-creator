//! The machine role on a pod or this computer: its launch grant, lifetime identity,
//! readiness receipt and rental lifecycle. Its contracts are the Hub's.
pub(crate) mod binary;
pub(crate) mod uv;
pub(crate) mod endpoint;
pub(crate) mod grant;
pub(crate) mod hub;
pub(crate) mod identity;
pub(crate) mod lifecycle;
pub(crate) mod net;
pub(crate) mod player;
pub(crate) mod probe;
pub(crate) mod provider;
pub(crate) mod pypi;
pub(crate) mod release;
pub(crate) mod receipt;
pub(crate) mod speed;
pub(crate) mod ssh;
pub(crate) mod stuck;
pub(crate) mod supervise;
pub(crate) mod toolchain;
pub(crate) mod update;
