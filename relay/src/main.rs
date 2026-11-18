//! `chaat-relay [--listen 127.0.0.1:8787] [--max-clients 1000]`
//!
//! Serve it over HTTPS on a public hostname (Caddy, nginx, a Cloudflare
//! tunnel, a cloud load balancer); Graph only posts to `https://` URLs.
//! See docs/realtime.md.

use std::net::SocketAddr;

#[path = "../../src/notices.rs"]
mod notices;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args = std::env::args().skip(1);
    let mut listen: SocketAddr = std::env::var("CHAAT_RELAY_LISTEN")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 8787)));
    let mut limits = chaat_relay::Limits::default();
    if let Some(max) = std::env::var("CHAAT_RELAY_MAX_CLIENTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|max| *max > 0)
    {
        limits.max_clients = max;
    }
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--licenses" => {
                print!("{}", notices::TEXT);
                return;
            }
            "--listen" => match args.next().and_then(|value| value.parse().ok()) {
                Some(address) => listen = address,
                None => {
                    eprintln!("--listen needs an address such as 0.0.0.0:8787");
                    std::process::exit(2);
                }
            },
            "--max-clients" => match args.next().and_then(|value| value.parse().ok()) {
                Some(max) if max > 0 => limits.max_clients = max,
                _ => {
                    eprintln!("--max-clients needs a positive number such as 1000");
                    std::process::exit(2);
                }
            },
            "--help" | "-h" => {
                println!(
                    "chaat-relay [--listen 127.0.0.1:8787] [--max-clients 1000] [--licenses]\nForwards Microsoft Graph change notifications to Chaat. Put it behind HTTPS.
Set CHAAT_RELAY_KEY to require an access key from clients."
                );
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let mut relay = chaat_relay::Relay::new(limits);
    // From the environment, not an argument: arguments show in process lists.
    match std::env::var("CHAAT_RELAY_KEY") {
        Ok(key) if !key.trim().is_empty() => match relay.with_key(&key) {
            Ok(keyed) => {
                relay = keyed;
                log::info!("clients must present the access key");
            }
            Err(reason) => {
                eprintln!("CHAAT_RELAY_KEY: {reason}");
                std::process::exit(2);
            }
        },
        _ => log::warn!("no CHAAT_RELAY_KEY: anyone who can reach the relay may connect"),
    }
    let listener = match tokio::net::TcpListener::bind(listen).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("could not listen on {listen}: {error}");
            std::process::exit(1);
        }
    };
    log::info!(
        "chaat-relay listening on {listen}, at most {} clients",
        limits.max_clients
    );
    let app = chaat_relay::router(relay);
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    if let Err(error) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
    {
        eprintln!("chaat-relay stopped: {error}");
        std::process::exit(1);
    }
}
