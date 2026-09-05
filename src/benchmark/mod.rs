pub mod cli;
mod report;

use crate::{
    movement::{MoveTarget, MovementPlugin},
    navigation::{NavGrid, NavigationPlugin, NavigationStats, PlanPaths, Route},
    scenario::Scenario,
    units::{Team, Unit, UnitPlugin},
};
use bevy::{prelude::*, time::TimeUpdateStrategy};
use cli::Config;
use report::{Report, Run, Sample, Stats};
use std::{
    collections::HashMap,
    error::Error,
    time::{Duration, Instant},
};

fn positions(world: &mut World) -> Vec<(u32, Vec3, bool)> {
    let mut units: Vec<_> = world
        .query::<(&Unit, &Transform, Has<MoveTarget>)>()
        .iter(world)
        .map(|(unit, transform, target)| (unit.0, transform.translation, target))
        .collect();
    units.sort_unstable_by_key(|(id, _, _)| *id);
    units
}

fn crossing_order(world: &mut World) -> (Vec<Vec3>, f64) {
    let start = Instant::now();
    let scenario = *world.resource::<Scenario>();
    let mut units: Vec<_> = world
        .query::<(Entity, &Unit, &Team)>()
        .iter(world)
        .map(|(entity, unit, team)| (entity, unit.0, team.0))
        .collect();
    units.sort_unstable_by_key(|(_, id, _)| *id);
    let grid = world.resource::<NavGrid>();
    let destinations: [Vec<Vec3>; 2] = std::array::from_fn(|team| {
        grid.formation(scenario.per_team(), scenario.center(1 - team), 2.5)
            .expect("benchmark formations fit the map")
    });
    let mut expected = Vec::with_capacity(units.len());
    for (entity, id, team) in units {
        let goal = destinations[team as usize][id as usize % scenario.per_team()];
        world
            .entity_mut(entity)
            .insert(MoveTarget(goal))
            .remove::<Route>();
        expected.push(goal.with_y(0.8));
    }
    (expected, start.elapsed().as_secs_f64() * 1000.0)
}

fn sample(world: &mut World, tick: usize, update_ms: f64, frame_ms: f64) -> Sample {
    let mut moving = 0;
    let mut pending = 0;
    for route in world
        .query_filtered::<Has<Route>, With<MoveTarget>>()
        .iter(world)
    {
        if route {
            moving += 1;
        } else {
            pending += 1;
        }
    }
    Sample {
        tick,
        update_ms,
        frame_ms,
        planning_ms: world.resource::<NavigationStats>().last_ms,
        moving,
        pending,
    }
}

fn finish(world: &mut World, expected: &[Vec3], run: &mut Run) {
    let positions = positions(world);
    let grid = world.resource::<NavGrid>();
    let mut checksum = 0xcbf29ce484222325_u64;
    let mut valid = positions.len() == expected.len();
    for ((id, position, moving), goal) in positions.iter().zip(expected) {
        if !moving && position.distance(*goal) < 0.001 {
            run.arrived += 1;
        }
        valid &= grid.is_walkable(*position) && grid.has_clearance(*position);
        for value in [
            *id,
            position.x.to_bits(),
            position.y.to_bits(),
            position.z.to_bits(),
        ] {
            checksum = (checksum ^ value as u64).wrapping_mul(0x100000001b3);
        }
    }
    let stats = world.resource::<NavigationStats>();
    run.planned = stats.planned;
    run.failed = stats.failed;
    run.checksum = checksum;
    run.pass = valid && run.failed == 0 && run.arrived == expected.len();
}

fn empty_run(per_team: usize, workload: &'static str, repeat: usize, mode: &'static str) -> Run {
    Run {
        per_team,
        workload,
        repeat,
        mode,
        samples: Vec::new(),
        order_ms: 0.0,
        planned: 0,
        failed: 0,
        arrived: 0,
        pass: false,
        valid_timing: true,
        checksum: 0,
    }
}

pub fn run_headless(config: &Config) -> Result<(), Box<dyn Error>> {
    let mut report = Report::new(config.output.clone())?;
    let sizes = if config.suite {
        vec![100, 500, 1000]
    } else {
        vec![config.per_team]
    };
    let workloads: Vec<&'static str> = if config.suite {
        vec!["idle", "crossing"]
    } else {
        vec!["crossing"]
    };
    let repeats = if config.suite { config.repeats } else { 1 };
    let mut checksums = HashMap::new();
    for per_team in sizes {
        for &workload in &workloads {
            for repeat in 1..=repeats {
                let mut app = App::new();
                app.add_plugins(MinimalPlugins)
                    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                        1.0 / 60.0,
                    )))
                    .insert_resource(Scenario::Benchmark { per_team })
                    .add_plugins((
                        NavigationPlugin,
                        UnitPlugin { visuals: false },
                        MovementPlugin,
                    ));
                app.finish();
                app.cleanup();
                for _ in 0..120 {
                    app.update();
                }
                *app.world_mut().resource_mut::<NavigationStats>() = NavigationStats::default();
                let mut run = empty_run(per_team, workload, repeat, "headless");
                let expected = if workload == "crossing" {
                    let (expected, ms) = crossing_order(app.world_mut());
                    run.order_ms = ms;
                    expected
                } else {
                    positions(app.world_mut())
                        .into_iter()
                        .map(|(_, position, _)| position)
                        .collect()
                };
                run.samples = Vec::with_capacity(config.ticks);
                for tick in 0..config.ticks {
                    let start = Instant::now();
                    app.update();
                    let update_ms = start.elapsed().as_secs_f64() * 1000.0;
                    run.samples
                        .push(sample(app.world_mut(), tick, update_ms, 0.0));
                }
                finish(app.world_mut(), &expected, &mut run);
                let previous = checksums
                    .entry((per_team, workload))
                    .or_insert(run.checksum);
                run.pass &= *previous == run.checksum;
                let stats = Stats::new(run.samples.iter().map(|sample| sample.update_ms));
                println!(
                    "{per_team} vs {per_team} {workload:8} repeat={repeat} mean={:.3}ms p95={:.3}ms p99={:.3}ms arrived={}/{} failed={} {}",
                    stats.mean,
                    stats.p95,
                    stats.p99,
                    run.arrived,
                    per_team * 2,
                    run.failed,
                    if run.pass { "PASS" } else { "FAIL" }
                );
                report.runs.push(run);
            }
        }
    }
    report.write()?;
    println!("Results: {}", report.directory.canonicalize()?.display());
    if report.runs.iter().any(|run| !run.pass) {
        return Err("Benchmark correctness checks failed; see report.md".into());
    }
    Ok(())
}

#[derive(Resource)]
pub struct VisualRun {
    config: Config,
    report: Report,
    started: Option<f64>,
    tick_start: Instant,
    expected: Vec<Vec3>,
    run: Run,
    completed: bool,
    occluded: bool,
}
impl VisualRun {
    pub fn label(&self) -> &'static str {
        if self.started.is_none() {
            "BENCHMARK: warming up"
        } else {
            "BENCHMARK: measuring"
        }
    }
}

pub fn add_graphical(app: &mut App, config: Config) -> Result<(), Box<dyn Error>> {
    let report = Report::new(config.output.clone())?;
    println!(
        "Graphical benchmark: {} vs {}, 3 s warmup + {} s measurement. Keep the window visible; input invalidates timing. Output: {}",
        config.per_team,
        config.per_team,
        config.seconds,
        report.directory.display()
    );
    let run = empty_run(config.per_team, "crossing", 1, "graphical");
    app.insert_resource(VisualRun {
        config,
        report,
        started: None,
        tick_start: Instant::now(),
        expected: Vec::new(),
        run,
        completed: false,
        occluded: false,
    })
    .add_systems(First, |mut state: ResMut<VisualRun>| {
        state.tick_start = Instant::now()
    })
    .add_systems(PreUpdate, track_occlusion)
    .add_systems(Update, start_graphical.before(PlanPaths))
    .add_systems(Last, record_graphical);
    Ok(())
}

fn start_graphical(world: &mut World) {
    let now = world.resource::<Time<Real>>().elapsed_secs_f64();
    if now < 3.0 || world.resource::<VisualRun>().started.is_some() {
        return;
    }
    let (expected, ms) = crossing_order(world);
    *world.resource_mut::<NavigationStats>() = NavigationStats::default();
    let mut state = world.resource_mut::<VisualRun>();
    state.expected = expected;
    state.run.order_ms = ms;
    state.started = Some(now);
}

fn track_occlusion(
    mut events: MessageReader<bevy::window::WindowOccluded>,
    mut state: ResMut<VisualRun>,
) {
    for event in events.read() {
        state.occluded = event.occluded;
        if event.occluded && state.started.is_some() {
            state.run.valid_timing = false;
        }
    }
}

fn record_graphical(world: &mut World) {
    world.resource_scope(|world, mut state: Mut<VisualRun>| {
        let Some(started) = state.started else {
            return;
        };
        if state.completed {
            return;
        }
        let time = world.resource::<Time<Real>>();
        let now = time.elapsed_secs_f64();
        let frame_ms = time.delta_secs_f64() * 1000.0;
        let update_ms = state.tick_start.elapsed().as_secs_f64() * 1000.0;
        let tick = state.run.samples.len();
        state
            .run
            .samples
            .push(sample(world, tick, update_ms, frame_ms));
        let focused = world
            .query::<&Window>()
            .iter(world)
            .all(|window| window.focused);
        let keys = world
            .resource::<ButtonInput<KeyCode>>()
            .get_pressed()
            .next()
            .is_some();
        let mouse = world
            .resource::<ButtonInput<MouseButton>>()
            .get_pressed()
            .next()
            .is_some();
        let wheel = !world
            .resource::<Messages<bevy::input::mouse::MouseWheel>>()
            .is_empty();
        let visible = !state.occluded;
        state.run.valid_timing &= focused && visible && !keys && !mouse && !wheel;
        if now - started >= state.config.seconds {
            state.completed = true;
            let mut run =
                std::mem::replace(&mut state.run, empty_run(0, "crossing", 0, "graphical"));
            finish(world, &state.expected, &mut run);
            let passed = run.pass;
            println!(
                "Graphical benchmark: arrived={}/{} failed={} correctness={} timing_valid={}",
                run.arrived,
                run.per_team * 2,
                run.failed,
                run.pass,
                run.valid_timing
            );
            state.report.runs.push(run);
            match state.report.write() {
                Ok(()) => {
                    println!("Results: {}", state.report.directory.display());
                    world.write_message(if passed {
                        AppExit::Success
                    } else {
                        AppExit::error()
                    });
                }
                Err(error) => {
                    eprintln!("Cannot write benchmark report: {error}");
                    world.write_message(AppExit::error());
                }
            }
        }
    });
}
