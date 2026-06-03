//! Parsing and validation of the user-supplied `config.json`.
//!
//! These structs mirror the JSON the user writes. Field names use `camelCase`
//! on the wire (see `rename_all`) and `deny_unknown_fields` so typos in the
//! config surface as actionable errors instead of being silently ignored.

pub mod watch;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Config {
    /// Allows `"$schema": "./config.schema.json"` for editor autocomplete.
    /// Only here so the key is accepted; never read by the app.
    #[allow(dead_code)]
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,

    #[serde(default = "default_title")]
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub theme: Theme,
    /// Fallback UI poll interval (seconds) used when live updates are unavailable.
    #[serde(default = "default_refresh")]
    pub refresh_interval: u64,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub groups: Vec<Group>,
}

fn default_title() -> String {
    "Homelab".to_string()
}
fn default_refresh() -> u64 {
    15
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Auto,
    Light,
    Dark,
}

/// Defaults applied to any check that does not override them.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Defaults {
    pub interval: u64,
    pub timeout: u64,
    pub warn_response_time_ms: u64,
    pub retention_days: u64,
    /// TLS checks go "degraded" when the certificate expires within this many days.
    #[serde(default = "default_warn_cert_days")]
    pub warn_cert_days: u64,
}

fn default_warn_cert_days() -> u64 {
    14
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            interval: 30,
            timeout: 5,
            warn_response_time_ms: 800,
            retention_days: 90,
            warn_cert_days: default_warn_cert_days(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Group {
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub services: Vec<Service>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Service {
    /// Stable identifier. Derived from the name if omitted.
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Icon URL (http/https) or a bundled icon name. Falls back to a letter avatar.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// One service can expose multiple endpoints (web UI, admin, metrics, ...).
    #[serde(default)]
    pub links: Vec<Link>,
    /// Zero or more health checks. No checks => a plain, unmonitored link tile.
    #[serde(default)]
    pub checks: Vec<Check>,
}

impl Service {
    /// The resolved, stable id for this service.
    pub fn id_or_slug(&self) -> String {
        self.id.clone().unwrap_or_else(|| slug(&self.name))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Link {
    pub label: String,
    pub url: String,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: CheckType,
    /// HTTP(S) URL, `host:port` for tcp, or `host` for ping.
    pub target: String,

    // --- HTTP-specific options ---
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub expect_status: Option<Vec<u16>>,
    #[serde(default)]
    pub expect_body_contains: Option<String>,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub insecure_skip_tls_verify: bool,

    // --- per-check overrides of `defaults` ---
    #[serde(default)]
    pub interval: Option<u64>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub warn_response_time_ms: Option<u64>,
    /// TLS-only: override `defaults.warnCertDays`.
    #[serde(default)]
    pub warn_cert_days: Option<u64>,
}

impl Check {
    pub fn interval(&self, d: &Defaults) -> u64 {
        self.interval.unwrap_or(d.interval)
    }
    pub fn timeout(&self, d: &Defaults) -> u64 {
        self.timeout.unwrap_or(d.timeout)
    }
    pub fn warn_ms(&self, d: &Defaults) -> u64 {
        self.warn_response_time_ms
            .unwrap_or(d.warn_response_time_ms)
    }
    pub fn warn_cert_days(&self, d: &Defaults) -> u64 {
        self.warn_cert_days.unwrap_or(d.warn_cert_days)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckType {
    Http,
    Tcp,
    Tls,
    Ping,
}

impl CheckType {
    pub fn label(self) -> &'static str {
        match self {
            CheckType::Http => "HTTP",
            CheckType::Tcp => "TCP",
            CheckType::Tls => "TLS",
            CheckType::Ping => "Ping",
        }
    }
}

/// Read, parse and validate the config file at `path`.
pub fn load(path: &Path) -> anyhow::Result<Config> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))?;
    // Tolerate a UTF-8 BOM, which some editors prepend on save.
    let raw = raw.trim_start_matches('\u{feff}');
    let config: Config = serde_json::from_str(raw)
        .with_context(|| format!("parsing config file {}", path.display()))?;
    config.validate().map_err(|errs| {
        anyhow!(
            "invalid config {}:\n  - {}",
            path.display(),
            errs.join("\n  - ")
        )
    })?;
    Ok(config)
}

impl Config {
    /// Validate semantic constraints not enforced by the type system.
    /// Returns all problems found so the user can fix them in one pass.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        let mut ids = std::collections::HashSet::new();

        for g in &self.groups {
            if g.name.trim().is_empty() {
                errs.push("a group has an empty name".to_string());
            }
            for s in &g.services {
                if s.name.trim().is_empty() {
                    errs.push(format!("a service in group '{}' has an empty name", g.name));
                }
                let id = s.id_or_slug();
                if id.is_empty() {
                    errs.push(format!(
                        "service '{}' resolves to an empty id; set an explicit 'id'",
                        s.name
                    ));
                } else if !ids.insert(id.clone()) {
                    errs.push(format!(
                        "duplicate service id '{}' (set a unique 'id' on service '{}')",
                        id, s.name
                    ));
                }
                let mut check_names = std::collections::HashSet::new();
                for c in &s.checks {
                    if c.name.trim().is_empty() {
                        errs.push(format!(
                            "service '{}' has a check with an empty name",
                            s.name
                        ));
                    } else if !check_names.insert(c.name.as_str()) {
                        errs.push(format!(
                            "service '{}' has duplicate check name '{}'",
                            s.name, c.name
                        ));
                    }
                    if c.target.trim().is_empty() {
                        errs.push(format!(
                            "check '{}' on service '{}' has an empty target",
                            c.name, s.name
                        ));
                    }
                    match c.kind {
                        CheckType::Http => {
                            if !(c.target.starts_with("http://")
                                || c.target.starts_with("https://"))
                            {
                                errs.push(format!(
                                    "http check '{}' on '{}': target must start with http:// or https://",
                                    c.name, s.name
                                ));
                            }
                        }
                        CheckType::Tcp => {
                            if parse_host_port(&c.target).is_none() {
                                errs.push(format!(
                                    "tcp check '{}' on '{}': target must be host:port",
                                    c.name, s.name
                                ));
                            }
                        }
                        CheckType::Tls => {
                            // host or host:port (port defaults to 443).
                            if c.target.contains(':') && parse_host_port(&c.target).is_none() {
                                errs.push(format!(
                                    "tls check '{}' on '{}': target must be host or host:port",
                                    c.name, s.name
                                ));
                            }
                        }
                        CheckType::Ping => {}
                    }
                }
            }
        }

        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }
}

/// Turn an arbitrary name into a URL/DOM-safe slug.
pub fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Parse a `host:port` target. The host may itself contain no colon (IPv6
/// literals are not supported in this minimal form).
pub fn parse_host_port(s: &str) -> Option<(String, u16)> {
    let (host, port) = s.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_basics() {
        assert_eq!(slug("Jellyfin"), "jellyfin");
        assert_eq!(slug("My Cool Service!"), "my-cool-service");
        assert_eq!(slug("  spaced  "), "spaced");
    }

    #[test]
    fn parse_host_port_works() {
        assert_eq!(
            parse_host_port("host:8096"),
            Some(("host".to_string(), 8096))
        );
        assert_eq!(
            parse_host_port("1.2.3.4:80"),
            Some(("1.2.3.4".to_string(), 80))
        );
        assert_eq!(parse_host_port("nohostport"), None);
        assert_eq!(parse_host_port(":80"), None);
        assert_eq!(parse_host_port("host:notaport"), None);
    }

    #[test]
    fn rejects_duplicate_ids() {
        let json = r#"{
            "groups": [{ "name": "g", "services": [
                { "name": "Same" }, { "name": "Same" }
            ]}]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_duplicate_check_names() {
        let json = r#"{
            "groups": [{ "name": "g", "services": [
                { "name": "Svc", "checks": [
                    { "name": "web", "type": "tcp", "target": "host:80" },
                    { "name": "web", "type": "tcp", "target": "host:81" }
                ]}
            ]}]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        let json = r#"{ "titel": "typo" }"#;
        assert!(serde_json::from_str::<Config>(json).is_err());
    }

    #[test]
    fn accepts_minimal_config() {
        let json = r#"{ "title": "Home", "groups": [] }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.validate().is_ok());
    }
}
