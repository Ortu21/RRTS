# Rust RTS — v0.0.14

A procedural 3D RTS prototype in Rust 2024 and Bevy 0.19. No external assets. Gameplay depends only on Bevy; benchmark reporting and machine metadata additionally use serde/serde_json and sysinfo (benchmark-only code paths).

HUD and window titles read the version from `Cargo.toml` (`env!("CARGO_PKG_VERSION")`); there is a single source of truth, no duplicated labels.

## Run and controls

Requirements: Rust stable 1.95+, Cargo, a native linker and a Metal/Vulkan/DirectX 12 capable graphics driver. The first Bevy build takes several minutes.

```sh
cargo run
```

The playground starts with one Commander per team on a 600 × 600 field with a deterministic random obstacle layout (fixed seed `MAP_SEED`, guaranteed 7 m lanes, capped coverage and connectivity). There is no prebuilt army or base: select your Commander, build Metal/Solar, then a Factory/LabT2, queue troops and fight. Commanders are armed (gun + missiles), build at 10 work/s within 26 m, and losing yours ends the match.

```sh
cargo run -- --ai skirmish
cargo run -- --ai both --ai-personality turtle --ai-personality2 rusher
```

`--ai skirmish` pits you (blue) against the red turtle AI; `--ai both` runs a 1v1 demo (blue rusher vs red turtle). Personalities are `turtle|rusher` per team (`--ai-personality` red, `--ai-personality2` blue, `--ai-team 0|1|2`). Demo keys: `+/-` speed, `0` reset 1x, `Space` pause. Score panel top-right. Match ends when a Commander dies (`R` restarts). The `VIEW & FOG` panel switches the played team (BLUE/RED) and toggles fog.

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
| Right click on ally | Guard it: follow at close range, engage threats near the ward (Shift queues it) |
| Shift + right click | Queue a move behind the live order |
| G | Arm attack-move targeting (toggle), then left-click a destination |
| Shift + G, Shift + click | Queue an attack-move behind the live order |
| P | Arm patrol targeting (toggle), then left-click appends loop waypoints |
| T | Arm guard targeting (toggle), then left-click a friendly unit |
| H | Hold selected units in place (acquire and fire, never chase) |
| S | Stop selected units |

Green rings show selected units. The playground HUD reports version, FPS, counts per team, queued orders and pending paths; the benchmark HUD adds projectiles, engaging units, kinds, queued/failed paths and targeting state.

### Constructions

Select a builder (Commander or Engineer): the BASE CONSTRUCTION menu appears. Pick Metal, Solar, Factory, Turret, Wall or LabT2, then left-click free ground (2 m grid snap). Only explicitly tasked builders march to the footprint edge and trickle work while inside their own build radius; any other order pauses the site. One active site per team, no refunds on cancel. Factories are rejected up front when all 12 exit doors are sealed (`Factory exits blocked`), so queued troops always have a door.

### Production

Select a completed Factory (tier 1) or LabT2 (tier 2): the factory panel enqueues producible units (Scout, HeavyTank, Artillery, Engineer, LightTank, plus HeavyTank2/Artillery2 from LabT2 only; Commander never queues; `MAX_QUEUE = 12`). Right-click free ground with no units selected to set the rally; new units march there. A finished product waits inside a surrounded factory (`blocked`) and retries about once per second, immediately when the rally changes — never teleports.

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
- `guard` (custom diagnostic, outside the suites): one patrolling ward per team, everyone else escorting at speed 7. With `--units-per-team 51` this keeps 100 guards following throughout the run.
- `ai-test`: representative full-game Playground (Commander start, seeded map, red AI, economy, constructions, production and fog). Default 7,200 ticks (~120 s sim), 2–3 repeats for determinism.
- `economy-benchmark` (separate industry measurement): 10×12 factory/Metal/Solar grid with busy queues.
- `ai-suite` / `ai-scenarios`: league AI-vs-AI and fast L1 micro-scenario battery.

```sh
./scripts/benchmark.sh quick --repeats 2
./scripts/benchmark.sh full --ticks 1200
cargo run --profile benchmark -- --benchmark --headless --workload crowd --units-per-team 2500 --ticks 900
cargo run --locked --profile benchmark -- --benchmark --headless --workload ai-test --ticks 1800 --repeats 2 --output /tmp/ai-test
cargo run --locked --profile benchmark -- --economy-benchmark --ticks 600 --repeats 2 --output /tmp/economy
cargo run --locked --profile benchmark -- --benchmark --headless --ai-scenarios --output /tmp/ai-scenarios
cargo run --locked --profile benchmark -- --benchmark --headless --ai-suite quick
```

Telemetry requiring full-world queries is sampled every 60 ticks outside the timed region; raw `App::update` timing is still stored for every tick. Each repeated case records median, p95/p99, min/max, median-of-means and median absolute deviation (MAD), alongside correctness and a deterministic state checksum.

Headless values are simulation CPU costs. Graphical frame intervals include rendering and presentation waits. Profile numbers are instrumented diagnostics with Bevy tracing. Never mix the three when comparing baselines.

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
./scripts/profile.sh --workload guard --units-per-team 51 --ticks 1800
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

### Fog overlay check

Headless fog data + concealment are covered by `cargo test fog` (including the overlay-refresh test that asserts the mesh uploads only when visibility or view changes). The visible overlay itself is validated manually in the graphical playground and `profile-visual.sh`.

### Results

Each benchmark run writes a new directory under `benchmark-results/`:

- `run.json`: canonical schema-versioned artifact with metadata, runs and aggregates.
- `report.md`: readable run and aggregate tables.
- `summary.csv`: one row per repetition.
- `samples.csv`: individual ticks and periodically sampled simulation state.
- `metadata.txt`: environment and suite parameters.

Output directories must be new, preventing accidental overwrite. `--help` lists all CLI options; `--skirmish` remains an alias for `--workload skirmish`.

### Full test suite and CI

```sh
./scripts/test-suite.sh
```

Runs formatting validation, `cargo check`, `cargo test`, strict Clippy, the quick benchmark suite, the full-game `ai-test`, the economy benchmark, the `ai-scenarios` battery and the headless fog check. Compiler/test logs and benchmark data are saved together; any failed correctness check returns a nonzero exit code. Results are ignored by Git.

`.github/workflows/ci.yml` runs the same gates on every PR with Cargo caching: `fmt + clippy`, `check + test + fog`, `quick benchmark` (movement/combat baselines) and `full-game` (`economy` + reduced `ai-test` + `ai-scenarios`). Logs and reports are kept as artifacts. Only functional correctness can fail CI; timing deltas stay descriptive.

Tests cover formation generation, picking, deterministic obstacle-safe paths, unreachable destinations, valid unique formation slots, body-aware planning (map edge, narrow passages, mixed hulls), retargeting, arrival at different frame rates, bounded planning work, blocked-factory retention, CLI validation and percentile calculations. Benchmark correctness checks all final destinations, remaining orders, route failures, obstacle clearance and repeated-run determinism.

## Architecture and limits

`main.rs` composes the Bevy plugins. `scenario` selects the playground or benchmark; `units` separates gameplay spawning from visual entities. `world`, `camera`, `selection`, `orders`, `movement` and `ui` retain their domains. `navigation` owns a 240 × 240 occupancy grid over the 600 × 600 map (`HALF_SIZE = 300`, `CELL_SIZE = 2.5`), deterministic Theta* any-angle routing with line-of-sight clearance, live congestion costs from the spatial index, body-aware hull clearance (`clearance_for(radius) = radius + 0.3`, `UNIT_CLEARANCE = 0.8` for the baked grid) and a budget of 128 path requests per frame (`PATHS_PER_FRAME = 128`, serial below 16). Destinations, formations, planning, swept collision and map clamps all agree on the same hull: scout-clear paths that a Commander cannot execute are rejected explicitly instead of stranding orders. Selected units show BAR-style order graphics: persistent order lines and destination markers in per-order colours, plus a fading flash of the actually planned route on every plan and replan. `benchmark` owns CLI parsing, automatic crossing orders, measurement and reports. `spatial` owns a uniform spatial hash rebuilt every frame, shared by local avoidance and target acquisition. `combat` owns health, weapons, order-driven targeting, homing projectiles and death handling; `orders` owns the `UnitOrder` intent (`Idle`, `Move`, `Attack`, `AttackMove`, `HoldPosition`, `Patrol`, `Guard`, `Build`) plus the temporary `AttackTarget` combat state and the `Chasing`/`HoldFire` locomotion markers. Beyond-All-Reason style rules: `AttackMove` stops to fight then resumes its route, `Move` fires on the march without chasing, holders and idle units defend in place. `Guard` follows a friendly ward at close range through the budgeted planner and engages enemies like attack-move; explicit `Attack` chases keep a synced `MoveTarget` so pursuit replans around obstacles instead of beelining through walls. Bodies face travel direction, turret barrels track their target (forward otherwise), and projectiles leave the muzzle. Units come in eight data-driven archetypes (`Scout`, `HeavyTank`, `Artillery`, `Commander`, `Engineer`, `LightTank`, `HeavyTank2`, `Artillery2`) defined in a single table (`units/archetype.rs`): health, speed, hull turn rate, turret traverse, aim tolerance, weapon, sight and body size. Hulls turn smoothly toward travel, turrets traverse toward locks, and fire is gated on barrel alignment. `economy` owns the 20 Hz streaming Metal/Energy accounts, build power and fair allocation; `production` owns tier-gated factory queues with blocked-exit retention and staggered retries; `structures` owns dynamic obstacles, swept motion collision (skipped for stationary units, snapshots updated in place) and construction sites; `fog` owns per-team shroud + visibility with a render overlay that refreshes only on change; `ai` owns scripted personalities, threat maps and the league/scenario harnesses. The map is dynamic: buildings add/remove nav obstacles and invalidate only intersecting routes.

Selection and orders remain ECS state. An order replaces the old route; movement waits for planning and consumes waypoints without overshoot. Blocked formation slots are relocated to unique free grid cells that fit the largest hull in the group. Unreachable routes stop and increment the HUD failure count. Grid slots are 2.5 units apart.

Playground and benchmarks apply soft local separation so units no longer stack; there are no physics bodies per unit. Benchmark scenarios isolate movement/combat for comparable numbers, while `ai-test`, `economy-benchmark` and `ai-scenarios` cover the complete gameplay (economy, constructions, production, fog and AI). There is no flow field, networking, save system or external asset loading. A batch of 2,000 requests spans at least 16 frames before every path has been planned.

Target platforms: macOS Apple Silicon, Windows x86_64 (MSVC tools), Linux x86_64 (C toolchain plus X11/Wayland and udev development libraries). Native validation is currently performed on macOS; Windows/Linux remain untested. `Cargo.lock` pins dependencies.

Possible follow-ups: avoidance cost tuning guided by `scripts/profile.sh`, wider benchmark size sweeps, learned AI opponents.
