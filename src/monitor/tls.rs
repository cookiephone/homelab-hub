//! TLS certificate-expiry checker.
//!
//! Connects and completes a handshake while accepting *any* certificate (so
//! self-signed homelab certs still report), then reports how long until the leaf
//! certificate expires:
//! - expired or connect/handshake failure  -> Down
//! - expires within `warn_days`             -> Degraded
//! - otherwise                              -> Up

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::crypto::ring::default_provider;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{
    ClientConfig, DigitallySignedStruct, Error as RustlsError, SignatureScheme,
};
use tokio_rustls::TlsConnector;

use crate::config::{parse_host_port, Check};
use crate::model::{CheckOutcome, Status};

use super::Checker;

pub struct TlsChecker {
    host: String,
    port: u16,
    timeout: Duration,
    warn_days: u64,
    config: Arc<ClientConfig>,
}

impl TlsChecker {
    pub fn new(check: &Check, timeout: Duration, warn_days: u64) -> anyhow::Result<Self> {
        // host or host:port (port defaults to 443).
        let (host, port) = parse_host_port(&check.target)
            .unwrap_or_else(|| (check.target.trim().to_string(), 443));

        let config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAny))
            .with_no_client_auth();

        Ok(Self {
            host,
            port,
            timeout,
            warn_days,
            config: Arc::new(config),
        })
    }
}

#[async_trait]
impl Checker for TlsChecker {
    async fn check(&self) -> CheckOutcome {
        let start = Instant::now();
        let addr = format!("{}:{}", self.host, self.port);

        let tcp = match timeout(self.timeout, TcpStream::connect(&addr)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return CheckOutcome::down(e.to_string()),
            Err(_) => return CheckOutcome::down("timed out"),
        };

        let server_name = match ServerName::try_from(self.host.clone()) {
            Ok(n) => n,
            Err(_) => return CheckOutcome::down("invalid TLS server name"),
        };

        let connector = TlsConnector::from(self.config.clone());
        let tls = match timeout(self.timeout, connector.connect(server_name, tcp)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return CheckOutcome::down(format!("TLS handshake failed: {e}")),
            Err(_) => return CheckOutcome::down("timed out"),
        };
        let latency = start.elapsed().as_millis() as u64;

        let (_, conn) = tls.get_ref();
        let leaf = match conn.peer_certificates().and_then(|c| c.first()) {
            Some(c) => c,
            None => return CheckOutcome::down("server presented no certificate"),
        };

        match days_until_expiry(leaf) {
            Ok(days) if days < 0 => {
                CheckOutcome::down(format!("certificate expired {} days ago", -days))
            }
            Ok(days) if (days as u64) <= self.warn_days => CheckOutcome {
                status: Status::Degraded,
                latency_ms: Some(latency),
                http_code: None,
                error: Some(format!("certificate expires in {days} days")),
                checked_at: None,
            },
            Ok(_) => CheckOutcome {
                status: Status::Up,
                latency_ms: Some(latency),
                http_code: None,
                error: None,
                checked_at: None,
            },
            Err(e) => CheckOutcome::down(format!("could not parse certificate: {e}")),
        }
    }
}

/// Whole days until the certificate's `notAfter` (negative if already expired).
fn days_until_expiry(der: &CertificateDer<'_>) -> anyhow::Result<i64> {
    use x509_parser::prelude::*;
    let (_, cert) = X509Certificate::from_der(der.as_ref()).map_err(|e| anyhow::anyhow!("{e}"))?;
    let not_after = cert.validity().not_after.timestamp(); // seconds since epoch
    let now = crate::store::now_ms() / 1000;
    Ok((not_after - now) / 86_400)
}

/// A verifier that accepts every certificate so we can read the expiry date of
/// self-signed certs too. Never used for anything trust-sensitive.
#[derive(Debug)]
struct AcceptAny;

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
        ]
    }
}
