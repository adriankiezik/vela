//! Vela — a clean-room Minecraft server written in Rust.
//!
//! Two halves joined by channels (see `docs/ARCHITECTURE.md`):
//!   * `net` — a tokio accept loop spawning a task per connection;
//!   * `sim` — a single `World` ticked at 20 TPS on its own thread.
//!
//! The network layer owns sockets; the simulation owns game state.

mod config;
mod ids;
mod inventory;
#[cfg(test)]
mod mca_diff;
mod net;
mod platform;
mod protocol;
mod registry;
mod runtime;
mod sim;
mod world;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use config::ServerConfig;

/// Ingress channel depth — packets from all connections funnel through here and
/// the sim drains the whole queue each tick, so this only needs to cover one
/// tick's worth of arrivals.
const INGRESS_CAP: usize = 1024;

#[tokio::main]
async fn main() {
    // Honors RUST_LOG (e.g. RUST_LOG=debug); defaults to info.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    // Probe this once up front: on a double-click launch (Explorer) the console
    // is ours and will close the instant we exit. We use it both to root runtime
    // files next to the exe (see `runtime::dir`) and to pause on the way out so
    // the user can read the startup log — notably the EULA notice.
    let owns_console = platform::owns_console();

    if let Err(e) = run().await {
        error!(error = %e, "server exited with error");
    }

    // Any exit path funnels here: for a double-click launch, hold the window open
    // until the user acknowledges. Terminal / panel launches (which don't own the
    // console) fall through and exit immediately, as before.
    if owns_console {
        platform::pause_before_exit();
    }
}

/// The server proper. Split out of `main` so every exit — the EULA gate, a bind
/// failure, `/stop`, Ctrl+C — passes back through `main`'s single pause-on-exit
/// point rather than closing the process (and a double-click console) directly.
async fn run() -> std::io::Result<()> {
    print_banner();

    // Load server.properties / eula.txt / player lists / server-icon.png from the
    // runtime directory (CWD for a shipped binary, the `target/…` exe dir under
    // `cargo run` — see `runtime::dir`), creating files as vanilla does. A return
    // of `None` means the EULA is not agreed: vanilla logs and exits here, so we
    // do the same.
    let config = match ServerConfig::load(runtime::dir()) {
        Some(config) => Arc::new(config),
        None => return Ok(()),
    };

    // The bind address comes from `server-ip`/`server-port`, but an explicit
    // command-line argument (used by the integration test and for ad-hoc runs)
    // overrides it.
    let addr = std::env::args().nth(1).unwrap_or_else(|| {
        let ip = config.properties.server_ip();
        let host = if ip.is_empty() { "0.0.0.0" } else { ip };
        format!("{host}:{}", config.properties.server_port())
    });

    // The simulation owns all game state and runs on a dedicated blocking thread
    // (it is CPU-bound and synchronous — not an async worker). Every connection
    // holds a clone of `to_sim` to deliver its decoded packets.
    //
    // `shutdown` is the cross-thread stop signal: the run loop watches it (as does
    // the `/stop` command), so raising it makes the sim save the world and exit.
    // `spawn_blocking` hands back a `JoinHandle` we can await, so we can wait for
    // that final save to finish before the process exits.
    let (to_sim, sim_rx) = mpsc::channel(INGRESS_CAP);
    let sim_config = Arc::clone(&config);
    let shutdown = Arc::new(AtomicBool::new(false));
    let sim_shutdown = Arc::clone(&shutdown);
    let mut sim_done = tokio::task::spawn_blocking(move || sim::run(sim_rx, sim_config, sim_shutdown));

    let listener = TcpListener::bind(&addr).await?;
    info!(
        %addr,
        protocol = protocol::PROTOCOL_VERSION,
        mc = protocol::VERSION_NAME,
        "Vela listening"
    );

    loop {
        tokio::select! {
            // Ctrl+C (SIGINT): ask the simulation to stop, then fall out of the
            // accept loop and wait for its final save below. Mirrors vanilla's
            // JVM shutdown hook, which saves all worlds before exiting.
            _ = tokio::signal::ctrl_c() => {
                info!("shutdown signal received; saving world");
                shutdown.store(true, Ordering::Relaxed);
                break;
            }
            // The simulation exited on its own — `/stop`, or the ingress channel
            // closed. It has already saved; tear the process down.
            res = &mut sim_done => {
                if let Err(e) = res {
                    error!(error = %e, "simulation thread ended abnormally");
                }
                info!("simulation stopped; server exiting");
                return Ok(());
            }
            // A transient accept error (e.g. EMFILE, ECONNABORTED) must not take
            // down the listener — log it and keep serving.
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(pair) => pair,
                    Err(e) => {
                        warn!(error = %e, "accept failed");
                        continue;
                    }
                };
                stream.set_nodelay(true).ok();
                let to_sim = to_sim.clone();
                let config = Arc::clone(&config);
                tokio::spawn(async move {
                    if let Err(e) = net::handle(stream, peer, to_sim, config).await {
                        error!(%peer, error = %e, "connection error");
                    }
                });
            }
        }
    }

    // Ctrl+C path: the sim is finishing its final save — wait for it so the world
    // is durably on disk before we exit (dropping sockets and the runtime).
    if let Err(e) = sim_done.await {
        error!(error = %e, "simulation thread ended abnormally during shutdown");
    }
    info!("world saved; shutdown complete");
    Ok(())
}

/// Clears the terminal and prints the startup banner: the "Vela" wordmark
/// rendered as FIGlet ASCII art (Slant font), a short tagline, a development-
/// build notice, and a couple of metadata rows (version / protocol). Everything
/// is horizontally centered against the current terminal width. Written straight
/// to stdout (not through `tracing`) so the art stays unadorned by log
/// formatting, with ANSI styling applied for a bit of polish.
fn print_banner() {
    use figlet_rs::FIGfont;
    use terminal_size::{terminal_size, Width};

    // ANSI: bold cyan for the wordmark, dim for supporting text, yellow for the
    // dev-build notice, reset after each piece.
    const WORDMARK: &str = "\x1b[1;36m";
    const DIM: &str = "\x1b[2m";
    const DEV: &str = "\x1b[1;33m";
    // Underlined cyan for the repository URL so terminals surface it as a link.
    const LINK: &str = "\x1b[4;36m";
    const RESET: &str = "\x1b[0m";

    // The Slant font, embedded at build time — no runtime file lookup.
    const SLANT_FONT: &str = include_str!("../assets/fonts/slant.flf");

    // Terminal width for centering; assume 80 columns when it can't be probed
    // (e.g. output redirected to a file or pipe).
    let width = terminal_size().map_or(80, |(Width(w), _)| w as usize);

    // Center a single (already-styled) line whose *visible* width is `visible`
    // by padding the left with spaces. ANSI codes have zero visible width, so
    // the pad is computed from the plain text, then applied to the styled line.
    let center = |visible: usize, styled: &str| {
        let pad = width.saturating_sub(visible) / 2;
        format!("{}{}", " ".repeat(pad), styled)
    };

    // Clear the screen and move the cursor home — the ANSI sequence works on any
    // VT100-compatible terminal (Linux, macOS, and Windows 10+ consoles).
    print!("\x1b[2J\x1b[H");

    println!();
    if let Ok(font) = FIGfont::from_content(SLANT_FONT).or_else(|_| FIGfont::standard()) {
        if let Some(art) = font.convert("Vela") {
            for line in art.to_string().lines() {
                println!("{}", center(line.len(), &format!("{WORDMARK}{line}{RESET}")));
            }
        }
    }

    let tagline = "A Minecraft server, written in Rust.";
    let dev = "⚙  development build — not for production use";
    let meta = format!(
        "MC {} · protocol {}",
        protocol::VERSION_NAME,
        protocol::PROTOCOL_VERSION,
    );
    let repo = "https://github.com/adriankiezik/vela";

    println!();
    println!("{}", center(tagline.len(), &format!("{DIM}{tagline}{RESET}")));
    // `dev` starts with a multi-byte glyph; count characters, not bytes.
    println!("{}", center(dev.chars().count(), &format!("{DEV}{dev}{RESET}")));
    println!("{}", center(meta.len(), &format!("{DIM}{meta}{RESET}")));
    println!("{}", center(repo.len(), &format!("{LINK}{repo}{RESET}")));
    println!();
}
