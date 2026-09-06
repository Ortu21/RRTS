pub mod industry;
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
            .add_systems(Startup, setup_hud)
            .add_systems(PostUpdate, update_hud.after(SelectionSystems));
    }
}

#[derive(Component)]
struct DebugHud;

type UnitStatus = (Has<Selected>, Has<MoveTarget>, Has<Route>);

fn setup_hud(mut commands: Commands, scenario: Res<crate::scenario::Scenario>) {
    commands.spawn((
        DebugHud,
        industry::BlocksMap,
        Interaction::None,
        Text::new("RTS Prototype v0.0.11\n\nFPS: ...\nUnits: 200\nSelected: 0"),
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
    commands.spawn((
        industry::BlocksMap, Interaction::None,
        Text::new("WASD / Arrows: pan   Q / E: rotate   Wheel: zoom\nLeft click / Drag: select   Shift: add / queue orders   Esc: clear   Right click: move / attack enemy / guard ally   G: attack-move, then left-click   P: patrol   T: guard, then left-click ally   H: hold   S: stop"),
        TextFont { font_size: FontSize::Px(15.0), ..default() },
        Node { position_type: PositionType::Absolute, bottom: px(16), left: px(16), ..default() },
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
    mut hud: Single<&mut Text, With<DebugHud>>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
) {
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
    let mut by_kind = [0; 5];
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
            "v0.0.11  |  {fps:.0} FPS\nBlue {blue} / Red {red}  |  Selected {selected}\nOrders queued {queued} / Paths pending {pending}{targeting}"
        );
        return;
    }
    hud.0 = format!(
        "RTS Prototype v0.0.11\n\n{mode}\nFPS: {fps:.0}\nUnits: {count}\nBlue: {blue}\nRed: {red}\n{kinds_line}\nProjectiles: {projectiles}\nEngaging: {engaging}\nSelected: {selected}\nQueued orders: {queued}\nPaths queued: {pending}\nPaths failed: {failed}{targeting}"
    );
}
