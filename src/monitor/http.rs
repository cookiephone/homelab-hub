//! HTTP(S) health checker.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::{Client, Method};

use crate::config::Check;
use crate::model::{CheckOutcome, Status};

use super::Checker;

pub struct HttpChecker {
    client: Client,
    method: Method,
    url: String,
    expect_status: Option<Vec<u16>>,
    expect_body_contains: Option<String>,
    warn_ms: u64,
}

impl HttpChecker {
    pub fn new(check: &Check, timeout: Duration, warn_ms: u64) -> anyhow::Result<Self> {
        let mut builder = Client::builder()
            .timeout(timeout)
            .user_agent(concat!("homelab-hub/", env!("CARGO_PKG_VERSION")));

        if check.insecure_skip_tls_verify {
            builder = builder.danger_accept_invalid_certs(true);
        }

        if let Some(headers) = &check.headers {
            let mut map = reqwest::header::HeaderMap::new();
            for (k, v) in headers {
                let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())?;
                let value = reqwest::header::HeaderValue::from_str(v)?;
                map.insert(name, value);
            }
            builder = builder.default_headers(map);
        }

        let method = match &check.method {
            Some(m) => Method::from_bytes(m.to_uppercase().as_bytes())?,
            None => Method::GET,
        };

        Ok(Self {
            client: builder.build()?,
            method,
            url: check.target.clone(),
            expect_status: check.expect_status.clone(),
            expect_body_contains: check.expect_body_contains.clone(),
            warn_ms,
        })
    }

    fn status_ok(&self, code: u16) -> bool {
        match &self.expect_status {
            Some(list) => list.contains(&code),
            None => (200..300).contains(&code),
        }
    }
}

#[async_trait]
impl Checker for HttpChecker {
    async fn check(&self) -> CheckOutcome {
        let start = Instant::now();
        let resp = self
            .client
            .request(self.method.clone(), &self.url)
            .send()
            .await;
        let latency = start.elapsed().as_millis() as u64;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                let msg = if e.is_timeout() {
                    "timed out".to_string()
                } else if e.is_connect() {
                    "connection failed".to_string()
                } else {
                    e.to_string()
                };
                return CheckOutcome::down(msg);
            }
        };

        let code = resp.status().as_u16();
        let mut status = if self.status_ok(code) {
            Status::Up
        } else {
            Status::Degraded
        };
        let mut error = if status == Status::Degraded {
            Some(format!("unexpected status {code}"))
        } else {
            None
        };

        // Optional body assertion (only meaningful if the status was acceptable).
        if status == Status::Up {
            if let Some(needle) = &self.expect_body_contains {
                match resp.text().await {
                    Ok(body) if body.contains(needle) => {}
                    Ok(_) => {
                        status = Status::Degraded;
                        error = Some(format!("body did not contain '{needle}'"));
                    }
                    Err(e) => {
                        status = Status::Degraded;
                        error = Some(format!("failed reading body: {e}"));
                    }
                }
            }
        }

        // Reachable and correct, but slow => degraded.
        if status == Status::Up && latency > self.warn_ms {
            status = Status::Degraded;
            error = Some(format!("slow response ({latency} ms)"));
        }

        CheckOutcome {
            status,
            latency_ms: Some(latency),
            http_code: Some(code),
            error,
            checked_at: None,
        }
    }
}
