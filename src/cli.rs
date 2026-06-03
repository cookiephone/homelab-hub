use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// A config-driven hub & status panel for your homelab.
#[derive(Parser, Debug)]
#[command(name = "homelab-hub", version, about, long_about = None)]
pub struct Cli {
    /// Path to the JSON config file describing your services.
    #[arg(
        short,
        long,
        env = "HUB_CONFIG",
        default_value = "config.json",
        global = true
    )]
    pub config: PathBuf,

    /// Path to the SQLite database used to store check history.
    #[arg(short, long, env = "HUB_DB", default_value = "hub.db", global = true)]
    pub db: PathBuf,

    /// Address to bind the web server to.
    #[arg(
        short,
        long,
        env = "HUB_BIND",
        default_value = "0.0.0.0:8080",
        global = true
    )]
    pub bind: SocketAddr,

    /// Serve existing history without running live checks, retention pruning or
    /// config hot-reload. Pair with `seed` to fire up a self-contained demo from
    /// the sample config without the (unreachable) example hosts turning red.
    #[arg(long, env = "HUB_DEMO", global = true)]
    pub demo: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Populate the database with synthetic check history for the configured
    /// services, so the dashboard has uptime bars, latency and events to show
    /// immediately. Then serve it read-only with `--demo`.
    Seed {
        /// Days of history to generate.
        #[arg(long, default_value_t = 30)]
        days: u64,

        /// Replace any existing history (clears the table first).
        #[arg(long)]
        reset: bool,
    },
}
