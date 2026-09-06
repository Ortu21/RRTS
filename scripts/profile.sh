#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

record=false
forward=()
for argument in "$@"; do
    if [[ "$argument" == "--record" ]]; then
        record=true
    else
        forward+=("$argument")
    fi
done

output="benchmark-results/profile-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$output"
trace="$output/trace.json"
run="$output/run"

cargo build --locked --profile profiling --features profile-chrome
binary="target/profiling/rust_rts"
TRACE_CHROME="$trace" RUST_LOG=info RRTS_RUN_PROFILE=profiling "$binary" \
    --benchmark --headless --workload skirmish --units-per-team 1000 --ticks 600 \
    "${forward[@]}" --profile-capture --output "$run"

report_args=(--profile-report "$trace" --profile-run "$run/run.json" --output "$output")
if [[ "$record" == true ]]; then
    report_args+=(--record-history)
fi
env -u TRACE_CHROME RRTS_RUN_PROFILE=profiling "$binary" "${report_args[@]}"
printf 'Trace: %s\nReport: %s\n' "$trace" "$output/profile-report.md"
