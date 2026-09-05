use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
};

use crate::{
    movement::MoveTarget,
    navigation::{NavigationStats, Route},
    selection::{Selected, SelectionSystems},
    units::Unit,
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

fn setup_hud(mut commands: Commands) {
    commands.spawn((
        DebugHud,
        Text::new("RTS Prototype v0.0.2\n\nFPS: ...\nUnits: 100\nSelected: 0"),
        TextFont {
            font_size: FontSize::Px(18.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            top: px(16),
            left: px(16),
            padding: UiRect::all(px(12)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.03, 0.05, 0.07, 0.85)),
    ));
    commands.spawn((
        Text::new("WASD / Arrows: pan   Q / E: rotate   Wheel: zoom\nLeft click / Drag: select   Shift: add   Esc: clear   Right click: move"),
        TextFont { font_size: FontSize::Px(15.0), ..default() },
        Node { position_type: PositionType::Absolute, bottom: px(16), left: px(16), ..default() },
    ));
}

fn update_hud(
    diagnostics: Res<DiagnosticsStore>,
    units: Query<UnitStatus, With<Unit>>,
    navigation: Res<NavigationStats>,
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
    let pending = units
        .iter()
        .filter(|(_, target, route)| *target && !*route)
        .count();
    let failed = navigation.failed;
    let mode = benchmark.as_ref().map_or("PLAYGROUND", |run| run.label());
    hud.0 = format!(
        "RTS Prototype v0.0.2\n\n{mode}\nFPS: {fps:.0}\nUnits: {count}\nSelected: {selected}\nPaths queued: {pending}\nPaths failed: {failed}"
    );
}
