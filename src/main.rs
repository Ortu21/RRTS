mod benchmark;
mod camera;
mod combat;
mod economy;
mod fog;
mod formation;
mod movement;
mod navigation;
mod orders;
mod picking;
mod production;
mod scenario;
mod selection;
mod spatial;
mod structures;
mod ui;
mod units;
mod world;

use benchmark::cli::{Config, HELP, Workload};
use bevy::prelude::*;
use scenario::Scenario;

fn main() -> std::process::ExitCode {
    let config = match Config::parse(std::env::args().skip(1)) {
        Ok(Some(config)) => config,
        Ok(None) => {
            println!("{HELP}");
            return std::process::ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("{error}\n\n{HELP}");
            return std::process::ExitCode::from(2);
        }
    };
    if config.history_report {
        return match benchmark::write_history_report(config.output.clone()) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if let (Some(trace), Some(run)) = (&config.profile_report, &config.profile_run) {
        return match benchmark::profile::write_profile_report(
            trace,
            run,
            config.output.clone(),
            config.record_history,
        ) {
            Ok(directory) => {
                println!("Profile report: {}", directory.display());
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if config.economy_benchmark {
        return match economy::benchmark::run(&config) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if config.headless {
        return match benchmark::run_headless(&config) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    let scene = if config.benchmark {
        if matches!(config.workload, Workload::Crowd | Workload::Skirmish) {
            Scenario::Skirmish {
                per_team: config.per_team,
            }
        } else {
            Scenario::Benchmark {
                per_team: config.per_team,
            }
        }
    } else {
        Scenario::Playground
    };
    let mut app = App::new();
    app.insert_resource(scene)
        .insert_resource(ClearColor(Color::srgb(0.12, 0.16, 0.20)))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: format!(
                    "RTS Prototype v{}{}",
                    env!("CARGO_PKG_VERSION"),
                    if config.benchmark {
                        " — Benchmark"
                    } else {
                        ""
                    }
                ),
                resolution: (1280, 800).into(),
                // VSync deliberately off everywhere: raw throughput, no
                // display quantization in the FPS readout.
                present_mode: bevy::window::PresentMode::AutoNoVsync,
                ..default()
            }),
            ..default()
        }))
        .add_plugins((
            navigation::NavigationPlugin,
            spatial::SpatialPlugin,
            world::WorldPlugin,
            camera::CameraPlugin,
            units::UnitPlugin { visuals: true },
            selection::SelectionPlugin,
            orders::OrderPlugin,
            combat::CombatPlugin,
            movement::MovementPlugin,
            ui::UiPlugin,
        ));
    if !config.benchmark {
        app.add_plugins((
            economy::EconomyPlugin,
            fog::FogPlugin { render: true },
            structures::StructuresPlugin { visuals: true },
            production::ProductionPlugin,
            ui::industry::IndustryUiPlugin,
        ));
    }
    if config.benchmark
        && let Err(error) = benchmark::add_graphical(&mut app, config)
    {
        eprintln!("{error}");
        return std::process::ExitCode::FAILURE;
    }
    match app.run() {
        AppExit::Success => std::process::ExitCode::SUCCESS,
        AppExit::Error(code) => std::process::ExitCode::from(code.get()),
    }
}
