#!/usr/bin/env pwsh
# Build, seed a demo database from the sample config, and serve it read-only.
# No reachable services required: the live probes are disabled so the made-up
# sample hosts don't all show as "down".
#
# Usage: pwsh scripts/demo.ps1 [bind-address]   (default 127.0.0.1:8080)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

$bind = if ($args.Count -ge 1) { $args[0] } else { '127.0.0.1:8080' }
$bin = Join-Path 'target' 'release' 'homelab-hub.exe'

cargo build --release
& $bin seed --config config.example.json --db demo.db --reset
Write-Host "Serving demo on http://$bind  (Ctrl+C to stop)"
& $bin --config config.example.json --db demo.db --demo --bind $bind
