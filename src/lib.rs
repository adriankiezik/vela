//! Library facade over the server internals.
//!
//! The server proper is a binary (`src/main.rs`) — that is where `main`, the
//! accept loop, and the pause-on-exit logic live, and it keeps its own module
//! tree. This `lib.rs` exists purely so that out-of-crate consumers that Cargo
//! only lets link against a *library* target — integration tests under `tests/`
//! and the Criterion benches under `benches/` — can reach the same modules. It
//! declares the identical module set as `main.rs`; the module *files* are shared,
//! so there is no duplicated logic, only a second crate root that re-declares
//! them as `pub`.
//!
//! Nothing in the shipped server depends on this file; it is dev-only scaffolding.

// The library root pulls in the whole tree but, unlike the binary, calls almost
// none of it — so most items look "unused" from here. Silence that: dead-code is
// meaningful in `main.rs`, not in a facade whose job is just to expose the API.
#![allow(dead_code, unused_imports)]

pub mod config;
pub mod ids;
pub mod inventory;
pub mod net;
pub mod platform;
pub mod protocol;
pub mod registry;
pub mod runtime;
pub mod sim;
pub mod world;
