//! TCP connect health checker. "Up" means the port accepts a connection.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::config::{parse_host_port, Check};
use crate::model::{CheckOutcome, Status};

use super::Checker;

pub struct TcpChecker {
    host: String,
    port: u16,
    timeout: Duration,
    warn_ms: u64,
}

impl TcpChecker {
    pub fn new(check: &Check, timeout: Duration, warn_ms: u64) -> Self {
        // Config validation guarantees `host:port`; fall back defensively.
        let (host, port) = parse_host_port(&check.target).unwrap_or((check.target.clone(), 0));
        Self {
            host,
            port,
            timeout,
            warn_ms,
        }
    }
}

#[async_trait]
impl Checker for TcpChecker {
    async fn check(&self) -> CheckOutcome {
        let addr = format!("{}:{}", self.host, self.port);
        let start = Instant::now();
        match timeout(self.timeout, TcpStream::connect(&addr)).await {
            Ok(Ok(_stream)) => {
                let latency = start.elapsed().as_millis() as u64;
                let status = if latency > self.warn_ms {
                    Status::Degraded
                } else {
                    Status::Up
                };
                CheckOutcome {
                    status,
                    latency_ms: Some(latency),
                    http_code: None,
                    error: None,
                    checked_at: None,
                }
            }
            Ok(Err(e)) => CheckOutcome::down(e.to_string()),
            Err(_) => CheckOutcome::down("timed out"),
        }
    }
}
