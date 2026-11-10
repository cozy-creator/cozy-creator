#![deny(unreachable_pub)]
// The machine API's error envelope is TensorD's error type; refusals are cold paths, so its size
// costs nothing worth boxing it for.
#![allow(clippy::result_large_err)]
// Every OS call goes through platform or a safe wrapper (xs-067).
#![forbid(unsafe_code)]
mod access;
mod activity;
mod adapter;
mod api;
mod boundary_json;
pub(crate) mod estimate;
mod call_memo;
mod catalog;
pub mod config;
mod child_launcher;
mod device_executor;
mod execution;
mod gpu_allocation;
mod gpu_service;
mod held_models;
mod gateway;
mod hub;
mod import_pages;
mod calls;
mod journal;
mod launch_identity;
mod local_source;
mod machine;
mod memo;
mod memory;
mod objects;
mod origin;
mod output_capacity;
mod pack;
use tensorfs::precision;
mod process;
pub(crate) mod private_io;
mod products;
mod seam;
mod published;
mod reclaim;
mod runs;
pub mod serve;
mod queue;
mod service;
mod stop;
mod stored;
mod storage;
mod triage;
mod trees;
mod uuid;
