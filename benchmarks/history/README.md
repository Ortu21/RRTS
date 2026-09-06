# Performance history

This directory contains compact, explicitly recorded benchmark and profiler summaries. Raw samples and Chrome Trace captures belong in the Git-ignored `benchmark-results/` directory.

Record a benchmark snapshot only from a stable, clean commit:

```sh
./scripts/benchmark.sh record
```

Generate a passive comparison report:

```sh
./scripts/benchmark.sh history
```

Snapshots are partitioned by artifact kind and machine key. Historical deltas are comparable only when machine, Cargo profile, suite schema, preset and case identity match. They are descriptive and never enforce a performance gate.

When a workload, measurement boundary or suite matrix changes materially, increment `SUITE_SCHEMA_VERSION` before recording the new baseline.
