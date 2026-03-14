mod app;
mod config;
mod error;
mod metrics;

mod api;
mod cache;
mod core;
mod embeddings;
mod semantic;
mod services;
mod types;
mod upstream;

use std::sync::Arc;
use tokio::signal::unix::{signal, SignalKind};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn parse_config_path() -> Option<String> {
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        if arg == "--config" {
            return args.next();
        }
    }

    None
}

fn parse_test_config() -> bool {
    std::env::args().any(|a| a == "--test-config")
}

fn parse_print_config() -> bool {
    std::env::args().any(|a| a == "--print-config")
}

fn resolve_config_path(explicit: Option<String>) -> Option<String> {
    if explicit.is_some() {
        return explicit;
    }

    let candidates = [
        "configs/ai-firewall.conf",
        "/etc/ai-firewall/ai-firewall.conf",
    ];

    for p in candidates {
        if std::path::Path::new(p).exists() {
            return Some(p.to_string());
        }
    }

    None
}

async fn config_reload_loop(
    state: Arc<app::AppState>,
    config_path: Option<String>,
) -> anyhow::Result<()> {
    let mut hup = signal(SignalKind::hangup())?;

    while hup.recv().await.is_some() {
        tracing::info!("received SIGHUP, reloading config");

        let Some(path) = config_path.as_deref() else {
            tracing::warn!("received SIGHUP but no config file path is known; reload skipped");
            continue;
        };

        match config::Config::from_file(path) {
            Ok(new_config) => {
                if let Err(e) = new_config.validate() {
                    tracing::error!("config validation failed: {}", e);
                    continue;
                }

                match app::build_runtime(&new_config).await {
                    Ok(new_chat_service) => {
                        {
                            let mut cfg = state.config.write().await;
                            *cfg = new_config.clone();
                        }

                        {
                            let mut svc = state.chat_service.write().await;
                            *svc = new_chat_service;
                        }

                        tracing::info!("config and runtime successfully reloaded from {}", path);
                    }
                    Err(e) => {
                        tracing::error!(
                            "config reload aborted: new runtime initialization failed: {}",
                            e
                        );
                    }
                }
            }
            Err(e) => {
                tracing::error!("config reload failed from {}: {}", path, e);
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,ai_firewall=debug,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let test_config = parse_test_config();
    let print_config = parse_print_config();

    let explicit_config_path = parse_config_path();
    let reload_config_path = resolve_config_path(explicit_config_path.clone());

    let cfg = config::Config::from_env_or_file(explicit_config_path.as_deref())?;
    cfg.validate()?;

    if print_config {
        println!("{:#?}", cfg);
        return Ok(());
    }

    if test_config {
        tracing::info!("configuration OK");

        app::build_runtime(&cfg).await?;

        tracing::info!("runtime dependencies initialized successfully");

        return Ok(());
    }

    let listen_addr = cfg.listen_addr.clone();
    let built = app::build_app(cfg).await?;

    let state = built.state.clone();

    tokio::spawn(async move {
        if let Err(e) = config_reload_loop(state, reload_config_path).await {
            tracing::error!("config reload loop failed: {}", e);
        }
    });

    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    tracing::info!("listening on {}", listen_addr);

    axum::serve(listener, built.router).await?;
    Ok(())
}
