//! ICMP ping checker. Compiled only with the `ping` feature; needs raw-socket
//! privileges (CAP_NET_RAW) at runtime.

use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
use surge_ping::{Client, Config as PingConfig, PingIdentifier, PingSequence};

use crate::config::Check;
use crate::model::{CheckOutcome, Status};

use super::Checker;

pub struct PingChecker {
    host: String,
    timeout: Duration,
    warn_ms: u64,
}

impl PingChecker {
    pub fn new(check: &Check, timeout: Duration, warn_ms: u64) -> Self {
        Self {
            host: check.target.trim().to_string(),
            timeout,
            warn_ms,
        }
    }
}

#[async_trait]
impl Checker for PingChecker {
    async fn check(&self) -> CheckOutcome {
        // surge-ping needs an IpAddr; resolve the host first.
        let ip: IpAddr = match tokio::net::lookup_host((self.host.as_str(), 0)).await {
            Ok(mut addrs) => match addrs.next() {
                Some(a) => a.ip(),
                None => return CheckOutcome::down("could not resolve host"),
            },
            Err(e) => return CheckOutcome::down(format!("DNS error: {e}")),
        };

        let client = match Client::new(&PingConfig::default()) {
            Ok(c) => c,
            Err(e) => return CheckOutcome::down(format!("icmp socket error: {e}")),
        };

        let mut pinger = client.pinger(ip, PingIdentifier(0)).await;
        pinger.timeout(self.timeout);

        match pinger.ping(PingSequence(0), &[0u8; 16]).await {
            Ok((_packet, rtt)) => {
                let latency = rtt.as_millis() as u64;
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
            Err(e) => CheckOutcome::down(format!("ping failed: {e}")),
        }
    }
}
