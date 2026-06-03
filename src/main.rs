mod cli;
mod config;
mod model;
mod monitor;
mod seed;
mod state;
mod store;
mod web;

use std::sync::Arc;

use anyhow::Context;
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let args = Cli::parse();

    // Subcommands run and exit; the default (no subcommand) starts the server.
    if let Some(Command::Seed { days, reset }) = args.command {
        return seed::run(&args.config, &args.db, days, reset).await;
    }

    let config = config::load(&args.config).context("failed to load configuration")?;
    tracing::info!(
        groups = config.groups.len(),
        "configuration loaded from {}",
        args.config.display()
    );

    let store = store::Store::open(&args.db)
        .await
        .context("failed to open database")?;
    tracing::info!("database ready at {}", args.db.display());

    // Warm the in-memory cache from the last known result of each check so the
    // dashboard isn't all "unknown" immediately after a restart.
    let warm = store.latest_all().await.unwrap_or_default();
    let retention_days = config.defaults.retention_days;

    let state = Arc::new(AppState::new(config, store.clone()));
    for (check_id, outcome) in warm {
        state.set_status(&check_id, outcome);
    }

    // In demo mode we serve the seeded history as-is: no live probes (the sample
    // hosts are unreachable and would flip everything to "down"), no retention
    // pruning of the synthetic rows, and no config hot-reload.
    if args.demo {
        tracing::info!(
            "demo mode: serving seeded history; live checks, retention and hot-reload disabled"
        );
    } else {
        monitor::spawn(state.clone());
        store::retention::spawn(store, retention_days);
        config::watch::spawn(state.clone(), args.config.clone());
    }

    let app = web::router(state.clone());
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("failed to bind {}", args.bind))?;

    tracing::info!("listening on http://{}", args.bind);
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,homelab_hub=info"));
    fmt().with_env_filter(filter).init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
