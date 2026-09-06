# Rust RTS — v0.0.9

A procedural 3D RTS prototype in Rust 2024 and Bevy 0.19. No external assets. Gameplay depends only on Bevy; benchmark reporting and machine metadata additionally use serde/serde_json and sysinfo (benchmark-only code paths).

## Run and controls

Requirements: Rust stable 1.95+, Cargo, a native linker and a Metal/Vulkan/DirectX 12 capable graphics driver. The first Bevy build takes several minutes.

```sh
cargo run
```

The playground contains 100 blue vs 100 red units on a 400 × 400 field with a deterministic random obstacle layout (fixed seed, guaranteed lanes and connectivity). Both squads spawn armed but standing: select units and issue orders manually (right-click move, G attack-move, H hold, S stop) to stage battles. Right-click orders route units into separate free destination slots.

| Control | Action |
|---|---|
| WASD / arrows | Pan relative to camera heading |
| Q / E | Rotate |
| Mouse wheel | Zoom |
| Left click / drag | Single / box selection |
| SHIFT | Add friendly units to selection |
| ESC | Clear selection, cancel drag and disarm targeting |
| Right click | Replace movement orders (plain Move, fires on the march) — or disarm targeting |
| Right click on enemy | Direct attack with focus fire (Shift queues it) |
| Shift + right click | Queue a move behind the live order |
| G | Arm attack-move targeting (toggle), then left-click a destination |
| Shift + G, Shift + click | Queue an attack-move behind the live order |
| P | Arm patrol targeting (toggle), then left-click appends loop waypoints |
| H | Hold selected units in place (acquire and fire, never chase) |
| S | Stop selected units |

Green rings show selected units. The HUD reports FPS, unit count per team, projectiles, engaging units, selection, queued paths and path failures.

## Benchmark and profiler

```sh
./scripts/benchmark.sh quick
./scripts/benchmark.sh full
```

The headless suite is the canonical performance baseline. It uses a fixed 1/60-second simulation step, 120 untimed warmup ticks and a dedicated optimized Cargo profile. `quick` covers one representative size for every workload with three repetitions; `full` expands sizes and uses five repetitions, plus a short 5,000-vs-5,000 combat stress case. Performance is always descriptive: only functional correctness can fail the command.

The workloads isolate different costs:

- `idle`: ECS scheduling and unchanged units.
- `crossing`: obstacle-aware path planning and movement without combat or avoidance.
- `crowd`: movement plus spatial-grid rebuild and local avoidance, without combat.
- `skirmish`: spatial lookup, targeting, movement, avoidance, projectiles and deaths.

```sh
./scripts/benchmark.sh quick --repeats 2
./scripts/benchmark.sh full --ticks 1200
cargo run --profile benchmark -- --benchmark --headless --workload crowd --units-per-team 2500 --ticks 900
```

Telemetry requiring full-world queries is sampled every 60 ticks outside the timed region; raw `App::update` timing is still stored for every tick. Each repeated case records median, p95/p99, min/max, median-of-means and median absolute deviation (MAD), alongside correctness and a deterministic state checksum.

### Passive history

```sh
./scripts/benchmark.sh record
./scripts/benchmark.sh history
```

`record` runs the full suite and copies its compact canonical JSON into `benchmarks/history/benchmark/<machine>/`. Recording is intentionally explicit, requires a clean Git commit and refuses duplicate snapshots. Raw samples and traces stay ignored under `benchmark-results/`; only compact summaries belong in history.

`history` creates `history.csv` and `history.md`, comparing each point only with the previous matching machine, Cargo profile, suite schema, preset and case. The deltas have no pass/fail threshold: they are evidence for human investigation, not an active regression gate.

### System profiler

```sh
./scripts/profile.sh --workload skirmish --units-per-team 1000 --ticks 600
```

The profiler builds with Bevy's native tracing instrumentation and writes:

- `trace.json`: the complete Chrome Trace event stream, openable in Perfetto or Chrome tracing tools.
- `profile-summary.json`: compact machine-readable totals and distributions per ECS system.
- `profile-report.md`: systems sorted by total measured time with calls, mean, p50, p95 and maximum.
- `run/run.json`: the matching workload and environment metadata.

Chrome Trace is used as the standard raw event format and timeline viewer; the project still owns workload orchestration, measurement boundaries, metadata, summaries and historical output. No marker systems are inserted into Bevy's ECS schedule. Profile numbers are instrumented diagnostics and must not be mixed with uninstrumented benchmark timings; parallel system totals can overlap.

Pass `--record` to `scripts/profile.sh` to retain the compact profile summary in Git history after profiling a clean commit. Raw traces are deliberately never tracked.

For a massive visual stress test with the same profiler (simulation + rendering + presentation in one frame):

```sh
./scripts/profile-visual.sh --units-per-team 2500 --seconds 60
```

Keep the window visible and focused and avoid all input; occlusion, focus loss or input invalidates graphical timing. At high unit counts expect GPU-bound frames: compare `frame_ms` against `update_ms` in the samples to separate CPU simulation cost from rendering cost. Scale up to `--units-per-team 10000` to find the rendering limit.

### Graphical benchmark

```sh
cargo run -- --benchmark
cargo run -- --benchmark --workload skirmish --units-per-team 1000 --seconds 30
```

The graphical mode remains useful for frame pacing and rendering observation. Keep its window visible and avoid input during measurement; focus, occlusion and input are tracked as timing-validity signals. VSync is disabled. Headless values are simulation CPU costs, while graphical frame intervals include rendering and presentation waits; neither measures GPU execution time directly.

### Results

Each benchmark run writes a new directory under `benchmark-results/`:

- `run.json`: canonical schema-versioned artifact with metadata, runs and aggregates.
- `report.md`: readable run and aggregate tables.
- `summary.csv`: one row per repetition.
- `samples.csv`: individual ticks and periodically sampled simulation state.
- `metadata.txt`: environment and suite parameters.

Output directories must be new, preventing accidental overwrite. `--help` lists all CLI options; `--skirmish` remains an alias for `--workload skirmish`.

### Full test suite

```sh
./scripts/test-suite.sh
```

Runs formatting validation, `cargo check`, `cargo test`, strict Clippy and the benchmark suite. Compiler/test logs and benchmark data are saved together; any failed correctness check returns a nonzero exit code. Results are ignored by Git.

Tests cover formation generation, picking, deterministic obstacle-safe paths, unreachable destinations, valid unique formation slots, retargeting, arrival at different frame rates, bounded planning work, CLI validation and percentile calculations. Benchmark correctness checks all final destinations, remaining orders, route failures, obstacle clearance and repeated-run determinism.

## Architecture and limits

`main.rs` composes the Bevy plugins. `scenario` selects the playground or benchmark; `units` separates gameplay spawning from visual entities. `world`, `camera`, `selection`, `orders`, `movement` and `ui` retain their domains. `navigation` owns a 160 × 160 occupancy grid over the 400 × 400 map, deterministic Theta* any-angle routing with line-of-sight clearance, live congestion costs from the spatial index, unit clearance and a budget of 32 path requests per frame. Selected units show BAR-style order graphics: persistent order lines and destination markers in per-order colours, plus a fading flash of the actually planned route on every plan and replan. `benchmark` owns CLI parsing, automatic crossing orders, measurement and reports. `spatial` owns a uniform spatial hash rebuilt every frame, shared by local avoidance and target acquisition. `combat` owns health, weapons, order-driven targeting, homing projectiles and death handling; `orders` owns the `UnitOrder` intent (`Idle`, `Move`, `Attack`, `AttackMove`, `HoldPosition`) plus the temporary `AttackTarget` combat state and the `Chasing`/`HoldFire` locomotion markers. Beyond-All-Reason style rules: `AttackMove` stops to fight then resumes its route, `Move` fires on the march without chasing, holders and idle units defend in place. Bodies face travel direction, turret barrels track their target (forward otherwise), and projectiles leave the muzzle. Units come in three data-driven archetypes (`Scout`, `Tank`, `Artillery`) defined in a single table (`units/archetype.rs`): health, speed, hull turn rate, turret traverse, aim tolerance, weapon and body size. Hulls turn smoothly toward travel, turrets traverse toward locks, and fire is gated on barrel alignment, so handling differs per kind. Adding a unit is a data row, not a systems change.

Selection and orders remain ECS state. An order replaces the old route; movement waits for planning and consumes waypoints without overshoot. Blocked formation slots are relocated to unique free grid cells. Unreachable routes stop and increment the HUD failure count. Grid slots are 2.5 units apart.

Playground units apply soft local separation so they no longer stack, but there are no physics bodies per unit and combat steering ignores obstacles. Benchmark scenarios stay movement-only (no avoidance, no combat) so their measurements remain comparable. There is no flow field, economy, AI, networking, save system or external asset loading. The map is static; dynamic obstacle rebuilding is not implemented. A batch of 2,000 requests spans at least 63 frames before every path has been planned.

Target platforms: macOS Apple Silicon, Windows x86_64 (MSVC tools), Linux x86_64 (C toolchain plus X11/Wayland and udev development libraries). Native validation is currently performed on macOS; Windows/Linux remain untested. `Cargo.lock` pins dependencies.

Possible follow-ups: patrol/guard orders on the same intent pipeline, avoidance cost tuning guided by `scripts/profile.sh`, wider benchmark size sweeps.
