//! Demo data generator.
//!
//! The sample [`config.example.json`](../config.example.json) points at made-up
//! hosts (`192.168.1.x`, `*.home.lab`) that won't resolve where you're evaluating
//! this, so live probes would just paint everything red. The `seed` subcommand
//! instead writes synthetic check history (uptime bars, latency, a few incidents,
//! a realistic up/degraded/down mix) straight into the database, which `--demo`
//! then serves read-only without the live monitor overwriting it (see `main.rs`).
//!
//! Generation is deterministic, so the demo looks the same every time. Each
//! service gets a "personality" (healthy, occasionally slow, recovered, currently
//! degrading, or currently down) and every check a stable latency baseline.

use std::path::Path;

use anyhow::Context;

use crate::config::{self, Check, CheckType, Defaults};
use crate::model::{CheckOutcome, Status};
use crate::store::{now_ms, Store};

const HOUR_MS: i64 = 3_600_000;
const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;

/// Generate `days` of synthetic history for every check in `config_path` and
/// write it to the SQLite database at `db_path`.
pub async fn run(config_path: &Path, db_path: &Path, days: u64, reset: bool) -> anyhow::Result<()> {
    let cfg = config::load(config_path).context("loading config to seed")?;
    let store = Store::open(db_path)
        .await
        .context("opening database to seed")?;

    if reset {
        let removed = store
            .prune(i64::MAX)
            .await
            .context("clearing existing history")?;
        if removed > 0 {
            tracing::info!("cleared {removed} existing rows");
        }
    } else if !store.latest_all().await?.is_empty() {
        anyhow::bail!(
            "database {} already contains history; pass --reset to replace it",
            db_path.display()
        );
    }

    let now = now_ms();
    let start = now - (days.max(1) as i64) * DAY_MS;
    let times = sample_times(start, now);

    let mut services = 0usize;
    let mut total_rows = 0u64;

    for group in &cfg.groups {
        for service in &group.services {
            if service.checks.is_empty() {
                continue; // a plain link tile with nothing to chart.
            }
            let profile = Profile::for_index(services);
            services += 1;

            let sid = service.id_or_slug();
            // Incidents are per-service so all of a service's checks go down
            // together, like a real outage.
            let incidents = profile.incidents(&mut Rng::seed(&sid), now);

            for check in &service.checks {
                let check_id = format!("{sid}::{}", check.name);
                let rows = generate(
                    check,
                    &cfg.defaults,
                    profile,
                    &incidents,
                    &times,
                    now,
                    &check_id,
                );
                total_rows += rows.len() as u64;
                store
                    .record_many(&check_id, &rows)
                    .await
                    .with_context(|| format!("seeding history for {check_id}"))?;
            }
        }
    }

    tracing::info!(
        "seeded {total_rows} rows across {services} services ({days}d) into {}",
        db_path.display()
    );
    eprintln!(
        "Seeded {total_rows} history rows for {services} services into {}.\n\
         Serve it with:\n  \
         homelab-hub --config {} --db {} --demo",
        db_path.display(),
        config_path.display(),
        db_path.display()
    );
    Ok(())
}

/// The behavioural archetype assigned to a service.
#[derive(Clone, Copy, PartialEq)]
enum Profile {
    /// Solid: essentially 100% up.
    Healthy,
    /// Up overall, but with recurring slow (degraded) stretches.
    Spiky,
    /// Had an outage earlier in the window; healthy now.
    Recovered,
    /// Latency creeping up; currently degraded.
    Degrading,
    /// In the middle of an outage right now; currently down.
    Outage,
}

impl Profile {
    /// Assign profiles to services in config order. Mostly healthy with a few
    /// accents, so a reasonably sized config shows lots of green plus one
    /// currently degraded and one currently down.
    fn for_index(i: usize) -> Profile {
        use Profile::*;
        const ORDER: &[Profile] = &[
            Healthy, Healthy, Spiky, Healthy, Recovered, Healthy, Degrading, Healthy, Healthy,
            Spiky, Outage, Healthy, Recovered, Healthy, Healthy, Healthy,
        ];
        ORDER[i % ORDER.len()]
    }

    /// Down windows `[start, end)` for a service with this profile.
    fn incidents(self, rng: &mut Rng, now: i64) -> Vec<(i64, i64)> {
        match self {
            Profile::Recovered => {
                // A resolved blip somewhere earlier in the window.
                let start = now - rng.range_i64(6 * HOUR_MS, 30 * HOUR_MS);
                let dur = rng.range_i64(20 * MINUTE_MS, 90 * MINUTE_MS);
                vec![(start, start + dur)]
            }
            Profile::Outage => {
                // Started a little while ago and still ongoing (extends past now).
                let start = now - rng.range_i64(8 * MINUTE_MS, 45 * MINUTE_MS);
                vec![(start, now + DAY_MS)]
            }
            _ => Vec::new(),
        }
    }
}

/// Build the timestamps to generate, denser towards now so both the 1h and 30d
/// windows look populated without writing a point for every real interval.
fn sample_times(start: i64, now: i64) -> Vec<i64> {
    let mut out = Vec::new();
    let mut t = start;
    while t <= now {
        out.push(t);
        let age = now - t;
        let step = if age <= 2 * HOUR_MS {
            MINUTE_MS
        } else if age <= 2 * DAY_MS {
            5 * MINUTE_MS
        } else {
            30 * MINUTE_MS
        };
        t += step;
    }
    // Guarantee a point right at "now" so the warmed current status is fresh.
    if out.last().copied().unwrap_or(0) < now - 1000 {
        out.push(now);
    }
    out
}

/// Produce the `(ts, outcome)` series for a single check.
fn generate(
    check: &Check,
    defaults: &Defaults,
    profile: Profile,
    incidents: &[(i64, i64)],
    times: &[i64],
    now: i64,
    seed_id: &str,
) -> Vec<(i64, CheckOutcome)> {
    let warn = check.warn_ms(defaults).max(1);
    let base = baseline_latency(check.kind, warn, &mut Rng::seed(seed_id));
    let mut jitter = Rng::seed(&format!("{seed_id}:jitter"));

    times
        .iter()
        .map(|&ts| {
            let outcome = if incidents.iter().any(|&(s, e)| (s..e).contains(&ts)) {
                down_outcome(check.kind)
            } else {
                let mut latency = jittered(base, &mut jitter);
                let mut degraded = false;

                // Spiky services have slow clusters; keep the most recent minutes
                // clean so the live pill still reads "up".
                if profile == Profile::Spiky && now - ts > 15 * MINUTE_MS {
                    let bucket = ts / (3 * HOUR_MS);
                    let mut br = Rng::seed(&format!("{seed_id}:spike:{bucket}"));
                    if br.frac() < 0.18 {
                        latency = warn + br.range_u64(40, 600);
                        degraded = true;
                    }
                }

                // Degrading services ramp up over the last few hours so they end
                // the window slow enough to read as degraded right now.
                if profile == Profile::Degrading {
                    let window = 3 * HOUR_MS;
                    let into = (ts - (now - window)).max(0);
                    let frac = (into as f64 / window as f64).min(1.0);
                    latency = (latency as f64 + frac * warn as f64 * 1.25) as u64;
                }

                if latency > warn {
                    degraded = true;
                }
                up_outcome(check.kind, latency, degraded, warn)
            };
            (ts, outcome)
        })
        .collect()
}

/// A plausible "normal" latency for a check kind, as a stable per-check value.
fn baseline_latency(kind: CheckType, warn: u64, rng: &mut Rng) -> u64 {
    match kind {
        CheckType::Http => rng.range_u64((warn / 8).max(20), (warn / 2).max(60)),
        CheckType::Tls => rng.range_u64(35, 160),
        CheckType::Tcp => rng.range_u64(1, 18),
        CheckType::Ping => rng.range_u64(1, 35),
    }
}

/// Apply ±~⅓ jitter around a baseline.
fn jittered(base: u64, rng: &mut Rng) -> u64 {
    let span = (base / 3).max(1);
    let delta = rng.range_u64(0, 2 * span) as i64 - span as i64;
    (base as i64 + delta).max(1) as u64
}

fn up_outcome(kind: CheckType, latency: u64, degraded: bool, warn: u64) -> CheckOutcome {
    CheckOutcome {
        status: if degraded {
            Status::Degraded
        } else {
            Status::Up
        },
        latency_ms: Some(latency),
        http_code: matches!(kind, CheckType::Http).then_some(200),
        error: degraded.then(|| format!("slow response: {latency} ms (warn ≥ {warn} ms)")),
        checked_at: None,
    }
}

fn down_outcome(kind: CheckType) -> CheckOutcome {
    let (http_code, error) = match kind {
        CheckType::Http => (Some(503), "HTTP request failed: connection refused"),
        CheckType::Tcp => (None, "connection refused"),
        CheckType::Tls => (None, "TLS handshake failed: connection reset"),
        CheckType::Ping => (None, "request timed out"),
    };
    CheckOutcome {
        status: Status::Down,
        latency_ms: None,
        http_code,
        error: Some(error.to_string()),
        checked_at: None,
    }
}

/// A tiny deterministic PRNG (splitmix64) seeded by a string, so the demo is
/// reproducible without pulling in an RNG dependency.
struct Rng(u64);

impl Rng {
    fn seed(s: &str) -> Self {
        // FNV-1a over the bytes gives a well-mixed 64-bit seed.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in s.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Rng(h)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Inclusive `i64` in `[lo, hi]` (or `lo` if the range is empty). Used for
    /// time offsets/durations.
    fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % (hi - lo + 1) as u64) as i64
    }

    /// Inclusive `u64` in `[lo, hi]` (or `lo` if the range is empty). Used for
    /// latencies.
    fn range_u64(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + self.next_u64() % (hi - lo + 1)
    }

    /// A float in `[0, 1)`.
    fn frac(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}
