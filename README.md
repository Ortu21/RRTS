# Rust RTS — v0.0.4

A procedural 3D RTS prototype in Rust 2024 and Bevy 0.19. No external assets or direct dependencies beyond Bevy.

## Run and controls

Requirements: Rust stable 1.95+, Cargo, a native linker and a Metal/Vulkan/DirectX 12 capable graphics driver. The first Bevy build takes several minutes.

```sh
cargo run
```

The playground contains 100 blue vs 100 red units on a 200 × 200 field with three walls and passages. Both squads open with attack-move orders toward the center: they advance, separate locally, acquire enemies through a shared spatial grid, chase, fire homing projectiles and resume their destination when engagements end. Right-click orders route units around the walls into separate free destination slots.

| Control | Action |
|---|---|
| WASD / arrows | Pan relative to camera heading |
| Q / E | Rotate |
| Mouse wheel | Zoom |
| Left click / drag | Single / box selection |
| SHIFT | Add friendly units to selection |
| ESC | Clear selection and cancel drag |
| Right click | Replace movement orders (plain Move, ignores enemies) |
| G | Attack-move selected units toward the center |
| H | Hold selected units in place (acquire and fire, never chase) |
| S | Stop selected units |

Green rings show selected units. The HUD reports FPS, unit count per team, projectiles, engaging units, selection, queued paths and path failures.

## Benchmark scene: 1,000 vs 1,000

```sh
cargo run -- --benchmark
```

Blue and red formations exchange sides through the passages. This is a **movement benchmark, without combat or unit collision avoidance**. The wider camera shows both armies. After 3 seconds of idle warmup, the test measures 30 seconds, writes results and exits. Keep the window visible and avoid input during measurement; unfocused frames, reported window occlusion or input invalidate graphical timing. VSync is disabled in every scene, so the HUD reports raw throughput instead of display-quantized rates. Keep the window visible even on platforms that do not report occlusion.

```sh
cargo run -- --benchmark --units-per-team 500 --seconds 40
cargo run -- --benchmark --headless --ticks 1800
cargo run -- --benchmark-suite
```

## Skirmish stress test: attack-move combat

```sh
cargo run -- --benchmark --headless --skirmish --units-per-team 1000
cargo run -- --benchmark --headless --skirmish --units-per-team 10000 --ticks 600
cargo run -- --benchmark --skirmish
cargo run -- --benchmark-suite --skirmish
```

`--skirmish` fields armed units on both map halves and orders every unit to attack-move at the enemy home side. The run measures full-combat simulation cost (spatial grid, avoidance, targeting, projectiles, deaths) plus kills, engaging units and projectiles per tick. Up to 10,000 units per team are supported; crossing formations above the free-cell count are rejected with an error instead of panicking. Skirmish correctness requires at least one kill, zero path failures, in-bounds finite positions, full survivor/kill accounting and matching checksums (health included) across repeats; obstacle clearance is waived because combat steering is local.

```sh
cargo run -- --benchmark --headless --skirmish --units-per-team 1000 --ticks 600 --profile-systems
```

`--profile-systems` prints a per-system-set cost table (grid rebuild, validate, acquire, resolve, plan, movement, chase, avoidance, weapon, projectile, death) without touching the CSV schema. Spans are approximate under parallel scheduling; read means, not single ticks.

The suite runs **100/500/1,000 units per team × idle/crossing × 3 repeats**, using fixed 1/60-second simulation steps. Adding `--skirmish` appends the skirmish workload at the same sizes. Each run excludes 120 warmup ticks and measures 1,800 ticks (30 simulated seconds). Headless execution runs as fast as possible, with no window, render entities, camera, selection or UI. Its timings are **simulation CPU costs, not graphical FPS**.

`--repeats`, `--ticks` and `--output <new-directory>` customize the suite. `--help` lists options. Output directories must be new to avoid overwriting earlier results. Short runs can fail arrival checks because movement has not finished.

For representative release measurements, use `cargo run --release -- ...`. Development builds optimize game code at level 1 and dependencies at level 3; reports record the profile. Compare the same profile, machine and mode.

### Results

Each run writes a new directory under `benchmark-results/`:

- `report.md`: readable tables, correctness and timing validity.
- `summary.csv`: mean, p50/p95/p99/max CPU time, graphical frame intervals, order generation cost, planning cost, route failures, arrivals and final-position checksum.
- `samples.csv`: individual ticks with timings, moving units and planning backlog.
- `metadata.txt`: version, platform, CPU count, simulation step and workload parameters.

Headless CPU timing wraps `App::update`; sample collection and final validation are outside that timer. Graphical CPU timing covers the main schedule and excludes render-app work; graphical frame intervals include rendering and presentation waits. Neither measures GPU execution time directly. Order generation is measured separately. Checksums must match across repeated suite cases.

### Full test suite

```sh
./scripts/test-suite.sh
```

Runs formatting validation, `cargo check`, `cargo test`, strict Clippy and the benchmark suite. Compiler/test logs and benchmark data are saved together; any failed correctness check returns a nonzero exit code. Results are ignored by Git.

Tests cover formation generation, picking, deterministic obstacle-safe paths, unreachable destinations, valid unique formation slots, retargeting, arrival at different frame rates, bounded planning work, CLI validation and percentile calculations. Benchmark correctness checks all final destinations, remaining orders, route failures, obstacle clearance and repeated-run determinism.

## Architecture and limits

`main.rs` composes the Bevy plugins. `scenario` selects the playground or benchmark; `units` separates gameplay spawning from visual entities. `world`, `camera`, `selection`, `orders`, `movement` and `ui` retain their domains. `navigation` owns an 80 × 80 occupancy grid, deterministic eight-neighbor A*, unit clearance and a budget of 32 path requests per frame. `benchmark` owns CLI parsing, automatic crossing orders, measurement and reports. `spatial` owns a uniform spatial hash rebuilt every frame, shared by local avoidance and target acquisition. `combat` owns health, weapons, order-driven targeting, homing projectiles and death handling; `orders` owns the `UnitOrder` intent (`Idle`, `Move`, `Attack`, `AttackMove`, `HoldPosition`) plus the temporary `AttackTarget` combat state. Holders acquire and fire without chasing; first-volley cooldowns are staggered deterministically per unit.

Selection and orders remain ECS state. An order replaces the old route; movement waits for planning and consumes waypoints without overshoot. Blocked formation slots are relocated to unique free grid cells. Unreachable routes stop and increment the HUD failure count. Grid slots are 2.5 units apart.

Playground units apply soft local separation so they no longer stack, but there are no physics bodies per unit and combat steering ignores obstacles. Benchmark scenarios stay movement-only (no avoidance, no combat) so their measurements remain comparable. There is no flow field, economy, AI, networking, save system or external asset loading. The map is static; dynamic obstacle rebuilding is not implemented. A batch of 2,000 requests spans at least 63 frames before every path has been planned.

Target platforms: macOS Apple Silicon, Windows x86_64 (MSVC tools), Linux x86_64 (C toolchain plus X11/Wayland and udev development libraries). Native validation is currently performed on macOS; Windows/Linux remain untested. `Cargo.lock` pins dependencies.

Possible follow-ups: patrol/guard orders on the same intent pipeline, avoidance cost tuning guided by `--profile-systems`, wider benchmark size sweeps.
