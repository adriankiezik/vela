//! Vela — a clean-room Minecraft server written in Rust.
//! Milestone 1: handshake + server-list status + login greeting (protocol 776).

mod connection;
mod protocol;
mod registries;
mod registry_tags;

use tokio::net::TcpListener;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Honors RUST_LOG (e.g. RUST_LOG=debug); defaults to info.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:25565".to_string());

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
        tokio::spawn(async move {
            if let Err(e) = connection::handle(stream, peer).await {
                error!(%peer, error = %e, "connection error");
            }
        });
    }
}
