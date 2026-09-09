//! UI presentazione: HUD + controlli nativi Bevy.
//!
//! - `shell` = layout top-bar/dock/sidebar, azioni sessione.
//! - `industry` = construction/production/ordini contestuali (solo presentazione, decisioni in sim).
//! - `debug` = overlay opt-in, mai usato dalla sim (l'AI non legge da qui).
//!
//! Vedi `DESIGN.md` + `PRODUCT.md` per vincoli.

pub mod debug;
pub mod industry;
pub mod shell;
use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
};

use crate::{
    combat::{AttackTarget, Projectile},
    movement::MoveTarget,
    navigation::{NavigationStats, Route},
    orders::{PendingOrder, UnitOrderQueue},
    selection::{Selected, SelectionSystems},
    units::{Team, Unit, UnitKind, archetype},
};

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(FrameTimeDiagnosticsPlugin::default())
            .init_resource::<industry::MapInput>()
            .init_resource::<debug::DebugSettings>()
            .init_resource::<debug::Inspected>()
            .init_resource::<shell::ShellState>()
            .init_resource::<crate::session::ViewState>()
            .add_systems(Startup, (setup_hud, shell::setup))
            .add_systems(PreUpdate, shell::actions.after(bevy::ui::UiSystems::Focus))
            .add_systems(
                PreUpdate,
                industry::capture_pointer
                    .after(shell::actions)
                    .after(bevy::ui::UiSystems::Focus),
            )
            .add_systems(Update, shell::scroll_panels)
            .add_systems(
                PostUpdate,
                (shell::sample, debug::draw_overlays, debug::draw_inspection)
                    .after(SelectionSystems),
            )
            .add_systems(PostUpdate, update_hud.after(SelectionSystems));
    }
}

#[derive(Component)]
struct DebugHud;

type UnitStatus = (Has<Selected>, Has<MoveTarget>, Has<Route>);

fn setup_hud(mut commands: Commands, scenario: Res<crate::scenario::Scenario>) {
    if matches!(*scenario, crate::scenario::Scenario::Playground) {
        return;
    }
    commands.spawn((
        DebugHud,
        industry::BlocksMap,
        Interaction::None,
        Text::new(concat!(
            "RTS Prototype v",
            env!("CARGO_PKG_VERSION"),
            "\n\nFPS: ...\nUnits: 200\nSelected: 0"
        )),
        TextFont {
            font_size: FontSize::Px(18.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            top: px(
                if matches!(*scenario, crate::scenario::Scenario::Playground) {
                    72
                } else {
                    16
                },
            ),
            left: px(16),
            padding: UiRect::all(px(12)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.03, 0.05, 0.07, 0.85)),
    ));
}

#[allow(clippy::too_many_arguments)]
fn update_hud(
    diagnostics: Res<DiagnosticsStore>,
    units: Query<UnitStatus, With<Unit>>,
    teams: Query<&Team, With<Unit>>,
    kinds: Query<&UnitKind, With<Unit>>,
    engaging: Query<(), (With<Unit>, With<AttackTarget>)>,
    projectiles: Query<(), With<Projectile>>,
    queued: Query<&UnitOrderQueue, With<Selected>>,
    navigation: Res<NavigationStats>,
    pending_order: Res<PendingOrder>,
    benchmark: Option<Res<crate::benchmark::VisualRun>>,
    hud: Option<Single<&mut Text, With<DebugHud>>>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
) {
    let Some(mut hud) = hud else {
        return;
    };
    *elapsed += time.delta_secs();
    if *elapsed < 0.1 {
        return;
    }
    *elapsed = 0.0;
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|fps| fps.smoothed())
        .unwrap_or(0.0);
    let count = units.iter().len();
    let selected = units.iter().filter(|(selected, _, _)| *selected).count();
    let blue = teams.iter().filter(|team| team.0 == 0).count();
    let red = teams.iter().filter(|team| team.0 != 0).count();
    let mut by_kind = [0; UnitKind::ALL.len()];
    for kind in &kinds {
        by_kind[kind.index()] += 1;
    }
    let kinds_line = UnitKind::ALL
        .iter()
        .map(|kind| format!("{}:{}", archetype(*kind).name, by_kind[kind.index()]))
        .collect::<Vec<_>>()
        .join("  ");
    let engaging = engaging.iter().len();
    let projectiles = projectiles.iter().len();
    let queued: usize = queued.iter().map(|queue| queue.0.len()).sum();
    let pending = units
        .iter()
        .filter(|(_, target, route)| *target && !*route)
        .count();
    let failed = navigation.failed;
    let mode = benchmark.as_ref().map_or("PLAYGROUND", |run| run.label());
    let targeting = match *pending_order {
        PendingOrder::None => String::new(),
        PendingOrder::AttackMove => {
            "\nAWAITING ATTACK-MOVE: left-click destination (ESC/right-click cancels)".to_string()
        }
        PendingOrder::Patrol => {
            "\nAWAITING PATROL: left-click adds a loop waypoint (ESC/right-click done)".to_string()
        }
        PendingOrder::Guard => {
            "\nAWAITING GUARD: left-click a friendly unit (ESC/right-click cancels)".to_string()
        }
    };
    if benchmark.is_none() {
        hud.0 = format!(
            "v{}  |  {fps:.0} FPS\nBlue {blue} / Red {red}  |  Selected {selected}\nOrders queued {queued} / Paths pending {pending}{targeting}",
            env!("CARGO_PKG_VERSION"),
        );
        return;
    }
    hud.0 = format!(
        "RTS Prototype v{}\n\n{mode}\nFPS: {fps:.0}\nUnits: {count}\nBlue: {blue}\nRed: {red}\n{kinds_line}\nProjectiles: {projectiles}\nEngaging: {engaging}\nSelected: {selected}\nQueued orders: {queued}\nPaths queued: {pending}\nPaths failed: {failed}{targeting}",
        env!("CARGO_PKG_VERSION"),
    );
}
