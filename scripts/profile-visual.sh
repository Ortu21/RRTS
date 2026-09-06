#!/usr/bin/env bash
# Massive VISUAL benchmark with Chrome Trace system profiling.
#
# Unlike scripts/profile.sh (headless CPU timings), this measures the full
# graphical frame: simulation + rendering + presentation. Keep the window
# visible and focused for the whole run and avoid all input; occlusion,
# focus loss or input invalidates graphical timing. VSync is off by design,
# so FPS is raw throughput. At 2500+ units per team expect GPU-bound frames;
# compare frame_ms against update_ms in samples to separate CPU from GPU.
#
# Example: ./scripts/profile-visual.sh --units-per-team 5000 --seconds 90
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

output="benchmark-results/profile-visual-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$output"
trace="$output/trace.json"
run="$output/run"

cargo build --locked --profile profiling --features profile-chrome
binary="target/profiling/rust_rts"
TRACE_CHROME="$trace" RUST_LOG=info RRTS_RUN_PROFILE=profiling "$binary" \
    --benchmark --workload skirmish --units-per-team 2500 --seconds 60 \
    "${forward[@]}" --profile-capture --output "$run"

report_args=(--profile-report "$trace" --profile-run "$run/run.json" --output "$output")
if [[ "$record" == true ]]; then
    report_args+=(--record-history)
fi
env -u TRACE_CHROME RRTS_RUN_PROFILE=profiling "$binary" "${report_args[@]}"
printf 'Trace: %s\nReport: %s\n' "$trace" "$output/profile-report.md"
