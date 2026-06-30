//! Vela — a clean-room Minecraft server written in Rust.
//!
//! Two halves joined by channels (see `docs/ARCHITECTURE.md`):
//!   * `net` — a tokio accept loop spawning a task per connection;
//!   * `sim` — a single `World` ticked at 20 TPS on its own thread.
//!
//! The network layer owns sockets; the simulation owns game state.

mod net;
mod protocol;
mod registries;
mod registry_tags;
mod sim;
mod world;

use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

/// Ingress channel depth — packets from all connections funnel through here and
/// the sim drains the whole queue each tick, so this only needs to cover one
/// tick's worth of arrivals.
const INGRESS_CAP: usize = 1024;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Honors RUST_LOG (e.g. RUST_LOG=debug); defaults to info.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:25565".to_string());

    // The simulation owns all game state and runs on its own OS thread (it is
    // CPU-bound and synchronous — not a tokio worker). Every connection holds a
    // clone of `to_sim` to deliver its decoded packets.
    let (to_sim, sim_rx) = mpsc::channel(INGRESS_CAP);
    std::thread::Builder::new()
        .name("vela-sim".to_string())
        .spawn(move || sim::run(sim_rx))
        .expect("spawn simulation thread");

    let listener = TcpListener::bind(&addr).await?;
    info!(
        %addr,
        protocol = protocol::PROTOCOL_VERSION,
        mc = protocol::VERSION_NAME,
        "Vela listening"
    );

    loop {
        // A transient accept error (e.g. EMFILE, ECONNABORTED) must not take
        // down the listener — log it and keep serving.
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                warn!(error = %e, "accept failed");
                continue;
            }
        };
        stream.set_nodelay(true).ok();
        let to_sim = to_sim.clone();
        tokio::spawn(async move {
            if let Err(e) = net::handle(stream, peer, to_sim).await {
                error!(%peer, error = %e, "connection error");
            }
        });
    }
}
