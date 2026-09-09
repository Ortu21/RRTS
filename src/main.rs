mod ai;
mod benchmark;
mod camera;
mod combat;
mod economy;
mod fog;
mod formation;
mod game_over;
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
mod view;
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
        if config.ai_suite.is_some() {
            return match ai::league::run_suite(&config) {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::ExitCode::FAILURE
                }
            };
        }
        if config.ai_scenarios {
            return match ai::scenarios::run_battery(&config) {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::ExitCode::FAILURE
                }
            };
        }
        if config.workload == Workload::AiTest {
            return match ai::harness::run(&config) {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::ExitCode::FAILURE
                }
            };
        }
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
        let control = view::SessionControl::from_cli(&config.ai, config.ai_team);
        let team = control.player_team().unwrap_or(0);
        let spectator = control.player_team().is_none();
        // Keep both personalities available for later handoffs, independently
        // from the set of teams currently controlled by bots.
        let ai_config = ai::AiConfig::versus(
            ai::Personality::from_name(
                if !spectator && config.ai != "off" && config.ai_team == 0 {
                    &config.ai_personality
                } else {
                    &config.ai_personality2
                },
            ),
            ai::Personality::from_name(&config.ai_personality),
            if spectator {
                ai::AiMode::Both
            } else {
                ai::AiMode::Skirmish
            },
        );
        app.insert_resource(control)
            .insert_resource(view::ViewState {
                team,
                fog_on: !spectator,
            })
            .insert_resource(ai_config)
            .add_plugins((
                economy::EconomyPlugin,
                fog::FogPlugin { render: true },
                structures::StructuresPlugin { visuals: true },
                production::ProductionPlugin,
                ui::industry::IndustryUiPlugin,
                game_over::GameOverPlugin,
                ai::AiPlugin,
                ai::debug::AiDebugPlugin,
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
