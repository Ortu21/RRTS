#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

command_name="${1:-quick}"
if (($# > 0)); then
    shift
fi

case "$command_name" in
    quick|full)
        RRTS_RUN_PROFILE=benchmark cargo run --locked --profile benchmark -- \
            --benchmark-suite "$command_name" "$@"
        ;;
    record)
        RRTS_RUN_PROFILE=benchmark cargo run --locked --profile benchmark -- \
            --benchmark-suite full --record-history "$@"
        ;;
    history)
        RRTS_RUN_PROFILE=benchmark cargo run --locked --profile benchmark -- \
            --history-report "$@"
        ;;
    *)
        printf 'Usage: %s {quick|full|record|history} [benchmark options]\n' "$0" >&2
        exit 2
        ;;
esac
