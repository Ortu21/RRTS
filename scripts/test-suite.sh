#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
output="${1:-benchmark-results/validation-$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$(dirname "$output")"
mkdir "$output"
trap 'printf "FAIL: see logs in %s\n" "$output" >&2' ERR
printf 'Running checks; logs: %s\n' "$output"
cargo fmt --check > "$output/fmt.log" 2>&1
cargo check > "$output/check.log" 2>&1
cargo test > "$output/tests.log" 2>&1
cargo clippy --all-targets -- -D warnings > "$output/clippy.log" 2>&1
RRTS_RUN_PROFILE=benchmark cargo run --locked --profile benchmark -- \
    --benchmark-suite quick --output "$output/benchmark" > "$output/benchmark.log" 2>&1
printf 'PASS: fmt, check, tests, clippy and quick benchmark correctness\n' | tee "$output/status.txt"
printf 'Report: %s/benchmark/report.md\n' "$output"
