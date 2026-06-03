#!/usr/bin/env bash
# Build, seed a demo database from the sample config, and serve it read-only.
# No reachable services required: the live probes are disabled so the made-up
# sample hosts don't all show as "down".
#
# Usage: scripts/demo.sh [bind-address]   (default 127.0.0.1:8080)
set -euo pipefail
cd "$(dirname "$0")/.."

bind="${1:-127.0.0.1:8080}"
bin=target/release/homelab-hub

cargo build --release
"$bin" seed --config config.example.json --db demo.db --reset
echo "Serving demo on http://$bind  (Ctrl+C to stop)"
exec "$bin" --config config.example.json --db demo.db --demo --bind "$bind"
