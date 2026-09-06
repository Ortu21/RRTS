# Guard validation — v0.0.10

Validated on 2026-09-06, macOS 26.6.2, Apple M1 Pro (8 cores, 16 GB),
Rust 1.96.0, Bevy 0.19.1. Measurements used the `profiling` Cargo profile
and Chrome Trace instrumentation; they are diagnostics, not a canonical
uninstrumented baseline. The working tree was dirty before the Guard commit.

## Engagement boundary

A guard acquires and retargets only enemies within its own AcquisitionRange
of both itself and the ward. An existing lock survives until the enemy is
beyond 1.5 times that range from either unit. For a tank these are 30/45 units.
The ward-centered acquisition filter runs during candidate selection, so a
closer excluded enemy cannot hide an eligible candidate. The gap prevents an
enemy just released at 45 from being reacquired until it returns within 30.
Dropping the lock resumes following the ward; it does not complete Guard.

The regression covers inclusive boundaries, retention in the hysteresis band,
release while the enemy is still close to the guard, no immediate reacquisition,
ward movement, excluded/eligible retargets, and restoring the ward MoveTarget.
The existing integration test covers following and completion when the ward dies.

## Sustained follow profile

```sh
./scripts/profile.sh --workload guard --units-per-team 51 --ticks 1800
```

This custom workload creates two patrolling wards and 100 guards, all moving
at speed 7 on the normal obstacle map. Teams patrol their own side so combat
losses do not shrink the follow workload. It is outside the historical suites.

| Measurement | Result |
|---|---:|
| Simulation | 1,800 ticks / 30 seconds |
| Units surviving / Guard orders retained | 102 / 100 |
| Successful route plans, including initial and ward patrol plans | 7,570 |
| Plans per frame / share of 32-plan capacity | 4.21 / 13.14% |
| Path failures | 0 |
| Planner system mean / p95 / maximum | 0.0632 / 0.1961 / 0.6356 ms |
| Instrumented App::update mean / p95 | 0.4905 / 0.6634 ms |
| Maximum sampled pending routes (sampled every 60 ticks) | 6 |

The planner cost is contained at this size. Keep the 4.0 refresh threshold;
these measurements do not justify adaptive tuning yet. The count includes
follow stop/resume as well as target drift and is not a pure drift-replan count.
The sampled queue maximum is not the instantaneous peak. Larger armies,
different terrain and other platforms remain unmeasured.

Local raw evidence (ignored by Git):
`benchmark-results/profile-20260906-171907/{trace.json,profile-report.md,run/run.json,run/samples.csv}`.

## Skirmish and gates

- Headless skirmish: 100 vs 100, 1,800 ticks, 158 kills, zero path failures,
  correctness PASS. Evidence: `benchmark-results/profile-20260906-171943/`.
- Graphical skirmish: 100 vs 100, 20 seconds after warmup, 130 kills,
  zero path failures, correctness and timing validity PASS.
  Evidence: `benchmark-results/guard-review-skirmish-visual/`.
- Initial tree: fmt/check/test/clippy PASS, 51 tests.
- Final tree: fmt/check/test/clippy PASS, 52 tests; clippy uses
  `--all-targets -- -D warnings`.

Beeline margin-stop, dense-grid behavior and Windows/Linux validation remain
outside this batch.
