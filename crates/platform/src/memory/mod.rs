//! A local event one process sets and its own threads wait on, alongside other handles.
pub use platform_signal::{Ready, Signal, Waitable, wait_any};
