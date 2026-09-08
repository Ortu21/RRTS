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
# Canonical baselines (movement/combat): timing descriptive only, only
# correctness (PASS/FAIL in report.md) can fail the command.
RRTS_RUN_PROFILE=benchmark cargo run --locked --profile benchmark -- \
    --benchmark-suite quick --output "$output/benchmark" > "$output/benchmark.log" 2>&1
# Representative full-game workloads (economy + constructions + fog + AI):
# ai-test is the Playground reale with Commander start, seeded map and red AI.
RRTS_RUN_PROFILE=benchmark cargo run --locked --profile benchmark -- \
    --benchmark --headless --workload ai-test --repeats 2 --output "$output/ai-test" > "$output/ai-test.log" 2>&1
# Sustainable industrial + AI scenario selection (fast, deterministic).
RRTS_RUN_PROFILE=benchmark cargo run --locked --profile benchmark -- \
    --economy-benchmark --ticks 600 --repeats 2 --output "$output/economy" > "$output/economy.log" 2>&1
RRTS_RUN_PROFILE=benchmark cargo run --locked --profile benchmark -- \
    --benchmark --headless --ai-scenarios --output "$output/ai-scenarios" > "$output/ai-scenarios.log" 2>&1
# Headless fog data + concealment check (unit tests already cover it, this
# keeps an explicit log); graphical overlay + frame pacing stay manual:
#   ./scripts/profile-visual.sh --units-per-team 2500 --seconds 60
# Headless values are simulation CPU costs, graphical frame intervals include
# rendering/presentation; profile numbers are instrumented diagnostics.
# Never mix the three when comparing baselines in benchmarks/history.
cargo test fog > "$output/fog.log" 2>&1
printf 'PASS: fmt, check, tests, clippy, quick benchmark, ai-test, economy and ai-scenarios correctness\n' | tee "$output/status.txt"
printf 'Report: %s/benchmark/report.md\n' "$output"
