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

use std::{sync::Arc, time::Duration};

use tokio::time::{sleep, Instant};
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
    #[cfg(unix)]
    let mut hup = {
        use tokio::signal::unix::{signal, SignalKind};
        signal(SignalKind::hangup())?
    };

    loop {
        #[cfg(unix)]
        {
            tokio::select! {
                _ = hup.recv() => {}
                _ = async {
                    while !state.shutdown.is_shutting_down() {
                        sleep(Duration::from_millis(200)).await;
                    }
                } => {
                    tracing::info!("stopping config reload loop due to shutdown");
                    break;
                }
            }
        }

        #[cfg(not(unix))]
        {
            let _ = &state;
            let _ = &config_path;
            break;
        }

        if state.shutdown.is_shutting_down() {
            tracing::info!("shutdown in progress, skipping config reload");
            break;
        }

        tracing::info!("received SIGHUP, reloading config");

        let Some(path) = config_path.as_deref() else {
            tracing::warn!("received SIGHUP but no config file path is known; reload skipped");
            continue;
        };

        match config::Config::from_file(path) {
            Ok(new_config) => {
                if let Err(e) = new_config.validate() {
                    tracing::error!("config validation failed during reload: {}", e);
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

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM");
            }
            _ = sigint.recv() => {
                tracing::info!("received SIGINT");
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install CTRL+C handler");
        tracing::info!("received CTRL+C");
    }
}

async fn graceful_shutdown(state: Arc<app::AppState>, drain_timeout: Duration) {
    shutdown_signal().await;

    tracing::info!("starting graceful shutdown");

    state.shutdown.begin_shutdown();

    let deadline = Instant::now() + drain_timeout;

    loop {
        let inflight = state.shutdown.inflight();

        if inflight == 0 {
            tracing::info!("all in-flight requests completed");
            break;
        }

        if Instant::now() >= deadline {
            tracing::warn!(
                "graceful shutdown timeout reached with {} in-flight request(s) remaining",
                inflight
            );
            break;
        }

        tracing::debug!(
            inflight_requests = inflight,
            "waiting for in-flight requests to drain"
        );

        sleep(Duration::from_millis(100)).await;
    }

    tracing::info!("graceful shutdown complete");
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

    let cfg = match config::Config::from_env_or_file(explicit_config_path.as_deref()) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::error!("startup aborted due to configuration error: {}", e);
            return Err(e);
        }
    };

    if let Err(e) = cfg.validate() {
        tracing::error!(
            "startup aborted due to configuration validation failure: {}",
            e
        );
        return Err(e);
    }

    if print_config {
        println!("{:#?}", cfg);
        return Ok(());
    }

    if test_config {
        tracing::info!("configuration OK");

        if let Err(e) = app::build_runtime(&cfg).await {
            tracing::error!("runtime dependency check failed: {}", e);
            return Err(e);
        }

        tracing::info!("runtime dependencies initialized successfully");
        return Ok(());
    }

    let listen_addr = cfg.listen_addr.clone();
    let shutdown_timeout = Duration::from_secs(cfg.graceful_shutdown_timeout_seconds);

    let built = match app::build_app(cfg).await {
        Ok(built) => built,
        Err(e) => {
            tracing::error!("startup aborted during runtime initialization: {}", e);
            return Err(e);
        }
    };
    let state = built.state.clone();

    tokio::spawn(async move {
        if let Err(e) = config_reload_loop(state, reload_config_path).await {
            tracing::error!("config reload loop failed: {}", e);
        }
    });

    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .map_err(|e| anyhow::anyhow!("failed to bind {}: {}", listen_addr, e))?;
    tracing::info!("listening on {}", listen_addr);

    axum::serve(listener, built.router)
        .with_graceful_shutdown(graceful_shutdown(built.state.clone(), shutdown_timeout))
        .await?;

    Ok(())
}
