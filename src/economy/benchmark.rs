//! Separate reproducible industrial workload; never changes legacy suites.
use crate::{
    benchmark::cli::Config,
    economy::{Economy, EconomyPlugin, balance::*},
    navigation::{CELL_SIZE, HALF_SIZE, NavGrid, NavigationPlugin},
    production::{Factory, ProductionPlugin},
    scenario::Scenario,
    structures::{StructuresPlugin, spawn_building},
    units::{Team, Unit, UnitKind, UnitPlugin},
};
use bevy::{prelude::*, time::TimeUpdateStrategy};
use std::time::{Duration, Instant};

pub fn run(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut reports = Vec::new();
    for repeat in 0..config.repeats {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                50,
            )))
            .insert_resource(Scenario::Benchmark { per_team: 0 })
            .add_plugins((
                NavigationPlugin,
                UnitPlugin { visuals: false },
                EconomyPlugin,
                StructuresPlugin { visuals: false },
                ProductionPlugin,
            ))
            .insert_resource(NavGrid::new(HALF_SIZE, CELL_SIZE, vec![]));
        app.finish();
        app.cleanup();
        app.update();
        let mut factories = Vec::new();
        for row in 0..10 {
            for col in 0..12 {
                let kind = [
                    BuildingKind::Factory,
                    BuildingKind::Metal,
                    BuildingKind::Solar,
                ][col % 3];
                let at = Vec3::new(-165.0 + col as f32 * 30.0, 0.0, -150.0 + row as f32 * 30.0);
                let entity = spawn_building(
                    &mut app.world_mut().commands(),
                    Team((row % 2) as u8),
                    kind,
                    at,
                    true,
                );
                if kind == BuildingKind::Factory {
                    factories.push(entity);
                }
            }
        }
        app.world_mut().flush();
        // Keep every factory busy for the full measurement without growing
        // unbounded queues. Refill outside timed App::update regions.
        let refill = |world: &mut World| {
            for e in &factories {
                let mut f = world.get_mut::<Factory>(*e).unwrap();
                while f.queue.len() < MAX_QUEUE {
                    f.enqueue(UnitKind::Tank);
                }
            }
        };
        for _ in 0..120 {
            refill(app.world_mut());
            app.update();
        }
        let mut samples = Vec::with_capacity(config.ticks);
        for _ in 0..config.ticks {
            refill(app.world_mut());
            let now = Instant::now();
            app.update();
            samples.push(now.elapsed().as_secs_f64() * 1000.0);
        }
        let units = app.world_mut().query::<&Unit>().iter(app.world()).count();
        let mut work = 0.0;
        let mut blocked = 0;
        for e in &factories {
            let f = app.world().get::<Factory>(*e).unwrap();
            work += f.queue.front().map_or(0.0, |j| j.project.done);
            blocked += usize::from(f.blocked);
        }
        let economy = app.world().resource::<Economy>();
        let stocks: Vec<_> = economy.0.iter().map(|(team, a)| (*team, a.stock)).collect();
        if economy
            .0
            .values()
            .any(|a| a.stock.iter().any(|v| !v.is_finite() || *v < 0.0))
            || units == 0
        {
            return Err("Industrial benchmark correctness failed".into());
        }
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        samples.sort_by(f64::total_cmp);
        let p95 = samples[(samples.len() as f64 * 0.95).ceil() as usize - 1];
        let result = serde_json::json!({"repeat": repeat + 1, "mean_ms": mean, "p95_ms": p95, "units": units, "active_work": work, "blocked_factories": blocked, "stocks": stocks});
        println!("Industrial: {result}");
        reports.push(result);
    }
    // Comparing deterministic outcomes also catches accidental query-order bias.
    if reports.windows(2).any(|r| {
        r[0]["units"] != r[1]["units"]
            || r[0]["active_work"] != r[1]["active_work"]
            || r[0]["stocks"] != r[1]["stocks"]
    }) {
        return Err("Industrial repeats diverged".into());
    }
    let report = serde_json::json!({"version": env!("CARGO_PKG_VERSION"), "factories": 40, "metal_generators": 40, "solars": 40, "teams": 2, "fixed_dt": 0.05, "warmup_ticks": 120, "ticks": config.ticks, "profile": "use --profile benchmark", "simulation": "economy + occupancy + production, open map, stationary outputs, no combat or rendering", "runs": reports});
    if let Some(directory) = &config.output {
        std::fs::create_dir(directory)?;
        std::fs::write(
            directory.join("economy.json"),
            serde_json::to_string_pretty(&report)?,
        )?;
    }
    Ok(())
}
