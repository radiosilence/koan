use koan_push_relay::Relay;
use koan_push_relay::config::Config;

#[tokio::main]
async fn main() {
    // The relay's own lines at KOAN_RELAY_LOG (info by default); everything
    // else at warn, whatever RUST_LOG says: at debug, the HTTP libraries log
    // request URLs, and Apple's hold device tokens.
    let level = std::env::var("KOAN_RELAY_LOG")
        .ok()
        .and_then(|l| l.parse().ok())
        .unwrap_or(log::LevelFilter::Info);
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Warn)
        .filter_module("koan_push_relay", level)
        .init();
    if let Err(e) = run().await {
        log::error!("{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let cfg = Config::load()?;
    let relay = Relay::new(&cfg)?;
    let listener = tokio::net::TcpListener::bind(cfg.listen)
        .await
        .map_err(|e| format!("binding {}: {e}", cfg.listen))?;
    let metrics = match cfg.metrics_listen {
        Some(addr) => Some(
            tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|e| format!("binding {addr}: {e}"))?,
        ),
        None => None,
    };
    log::info!("relaying on {}", cfg.listen);
    koan_push_relay::serve(relay, listener, metrics, shutdown()).await
}

/// Ctrl-C, or the SIGTERM a container is stopped with.
async fn shutdown() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("installing the SIGTERM handler");
        tokio::select! {
            _ = interrupt => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = interrupt.await;
    log::info!("stopping");
}
