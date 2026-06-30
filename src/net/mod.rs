//! The network layer: per-connection async tasks that own sockets.
//!
//! It never touches game state. Pre-Play states (`connection`) are handled
//! inline; the Play phase (`play`) bridges the socket to the simulation through
//! the `sim::bridge` channels. See `docs/ARCHITECTURE.md`.

pub mod connection;
mod frame;
mod play;

pub use connection::handle;
