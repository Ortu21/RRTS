//! Persistent native Bevy controls. Pointer capture spans press through release.
use crate::{
    camera::RtsCamera,
    combat::Health,
    economy::balance::*,
    orders::PendingOrder,
    picking::ground_position,
    production::Factory,
    selection::{Selected, SelectionSystems},
    session::ViewState,
    structures::{self, Building, Construction, Placement},
    units::{Builder, CollisionRadius, Team, Unit, UnitKind, archetype},
};
use bevy::{prelude::*, transform::TransformSystems, window::PrimaryWindow};

#[derive(Resource)]
pub struct MapInput {
    pub blocked: bool,
    captured: bool,
    pub commands_allowed: bool,
}
impl Default for MapInput {
    fn default() -> Self {
        Self {
            blocked: false,
            captured: false,
            commands_allowed: true,
        }
    }
}
impl MapInput {
    pub fn cancel_gesture(&mut self) {
        self.captured = false;
        self.blocked = true;
    }
}
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct MapInputSystems;
#[derive(Component)]
pub struct BlocksMap;
pub fn map_input_allowed(input: Res<MapInput>) -> bool {
    !input.blocked && input.commands_allowed
}
#[derive(Component, Clone, Copy)]
enum Action {
    Build(BuildingKind),
    Produce(UnitKind),
    Cancel(usize),
    CancelSite,
    Command(u8),
}
/// Marker on the three BASE CONSTRUCTION buttons: shown only while a builder
/// of the viewed team (Commander/Engineer) is selected — click builder, menu appears.
#[derive(Component)]
struct BuildButton;
#[derive(Component)]
struct BaseText;
#[derive(Component)]
struct ContextText;
#[derive(Component)]
struct QueueLabel(usize);
#[derive(Component)]
struct FactoryPanel;
#[derive(Component)]
struct SiteButton;
pub struct IndustryUiPlugin;
impl Plugin for IndustryUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewState>()
            .add_systems(
                PostUpdate,
                (actions, set_factory_rally, placement_and_rally)
                    .chain()
                    .in_set(MapInputSystems)
                    .after(TransformSystems::Propagate)
                    .before(SelectionSystems),
            )
            .add_systems(
                PostUpdate,
                (update_text, structures::draw_rallies).after(SelectionSystems),
            );
    }
}
fn font(size: f32) -> TextFont {
    TextFont {
        font_size: FontSize::Px(size),
        ..default()
    }
}
fn button(parent: &mut ChildSpawnerCommands, action: Action, label: String) {
    let mut entity = parent.spawn((
        Button,
        BlocksMap,
        action,
        Node {
            padding: UiRect::all(px(7)),
            min_height: px(30),
            ..default()
        },
        BackgroundColor(Color::srgb(0.14, 0.23, 0.29)),
    ));
    // Build buttons form the contextual builder menu (see update_text).
    if matches!(action, Action::Build(_)) {
        entity.insert(BuildButton);
    }
    entity.with_children(|p| {
        p.spawn((Text::new(label), font(14.0)));
    });
}
pub fn spawn_dock(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            flex_grow: 1.0,
            min_height: px(0),
            column_gap: px(12),
            ..default()
        })
        .with_children(|p| {
            p.spawn((
                BlocksMap,
                Interaction::None,
                Node {
                    width: px(210),
                    flex_shrink: 0.0,
                    overflow: Overflow::scroll_y(),
                    ..super::shell::column()
                },
                ScrollPosition::default(),
            ))
            .with_children(|p| {
                p.spawn((super::shell::Label::Selection, Text::default(), font(14.0)));
            });
            p.spawn((
                BlocksMap,
                Interaction::None,
                Node {
                    flex_grow: 1.0,
                    flex_basis: px(0),
                    min_width: px(0),
                    overflow: Overflow::scroll_y(),
                    ..super::shell::column()
                },
                ScrollPosition::default(),
            ))
            .with_children(|p| {
                p.spawn((BaseText, Text::default(), font(13.0)));
                p.spawn(Node {
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: px(5),
                    row_gap: px(5),
                    ..default()
                })
                .with_children(|p| {
                    for kind in BuildingKind::ALL {
                        let s = kind.stats();
                        button(
                            p,
                            Action::Build(kind),
                            format!(
                                "{}\n{}M {}E",
                                s.name, s.cost.resources[0], s.cost.resources[1]
                            ),
                        );
                    }
                    for (i, name) in [
                        "Attack-move [G]",
                        "Patrol [P]",
                        "Guard [T]",
                        "Hold [H]",
                        "Stop [X]",
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        button(p, Action::Command(i as u8), name.into());
                    }
                });
                p.spawn((
                    SiteButton,
                    Node {
                        display: Display::None,
                        ..default()
                    },
                ))
                .with_children(|p| button(p, Action::CancelSite, "Cancel site | no refund".into()));
                p.spawn((
                    FactoryPanel,
                    Node {
                        display: Display::None,
                        flex_wrap: FlexWrap::Wrap,
                        column_gap: px(5),
                        row_gap: px(5),
                        ..default()
                    },
                ))
                .with_children(|p| {
                    for kind in UnitKind::PRODUCIBLE {
                        let c = unit_cost(kind);
                        button(
                            p,
                            Action::Produce(kind),
                            format!(
                                "{}\n{}M {}E",
                                archetype(kind).name,
                                c.resources[0],
                                c.resources[1]
                            ),
                        );
                    }
                });
            });
            p.spawn((
                QueuePanel,
                BlocksMap,
                Interaction::None,
                Node {
                    width: px(180),
                    flex_shrink: 0.0,
                    overflow: Overflow::scroll_y(),
                    ..super::shell::column()
                },
                ScrollPosition::default(),
            ))
            .with_children(|p| {
                p.spawn((Text::new("PRODUCTION QUEUE"), font(13.0)));
                for i in 0..MAX_QUEUE {
                    p.spawn((
                        Button,
                        BlocksMap,
                        Action::Cancel(i),
                        Node {
                            display: Display::None,
                            padding: UiRect::all(px(5)),
                            flex_shrink: 0.0,
                            ..default()
                        },
                        BackgroundColor(super::shell::BUTTON),
                    ))
                    .with_children(|p| {
                        p.spawn((QueueLabel(i), Text::default(), font(13.0)));
                    });
                }
            });
        });
    parent.spawn((ContextText, Text::default(), font(13.0)));
}
#[derive(Component)]
struct QueuePanel;
pub fn capture_pointer(
    mouse: Res<ButtonInput<MouseButton>>,
    mut placement: Option<ResMut<Placement>>,
    interactions: Query<&Interaction, With<BlocksMap>>,
    mut input: ResMut<MapInput>,
    match_result: Option<Res<crate::game_over::MatchResult>>,
    control: Option<Res<crate::session::SessionControl>>,
) {
    input.commands_allowed = !match_result.is_some_and(|r| r.over)
        && control.as_deref().is_none_or(|c| c.player_team().is_some());
    if !input.commands_allowed
        && let Some(placement) = placement.as_deref_mut()
    {
        placement.kind = None;
        placement.builders.clear();
    }
    let pointer = interactions.iter().any(|i| *i != Interaction::None);
    if pointer && (mouse.just_pressed(MouseButton::Left) || mouse.just_pressed(MouseButton::Right))
    {
        input.captured = true;
    }
    input.blocked =
        pointer || input.captured || placement.as_deref().is_some_and(|p| p.kind.is_some());
    if !mouse.pressed(MouseButton::Left) && !mouse.pressed(MouseButton::Right) {
        input.captured = false;
    }
}
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn actions(
    mut commands: Commands,
    buttons: Query<(&Interaction, &Action), (Changed<Interaction>, With<Button>)>,
    selected: Query<(Entity, &Team, Has<Construction>), (With<Selected>, With<Building>)>,
    selected_builders: Query<(Entity, &Team), (With<Selected>, With<Unit>, With<Builder>)>,
    mut factories: Query<&mut Factory>,
    mut placement: ResMut<Placement>,
    mut pending: ResMut<PendingOrder>,
    mut input: ResMut<MapInput>,
    view: Res<ViewState>,
    selected_units: Query<Entity, (With<Selected>, With<Unit>)>,
) {
    if !input.commands_allowed {
        return;
    }
    let selected = selected
        .iter()
        .filter(|(_, t, _)| t.0 == view.team)
        .min_by_key(|r| r.0.to_bits());
    // Builders tasked by this placement: frozen at button press, march on
    // confirm. Deterministic order for multi-selection. Any builder kind
    // (Commander / Engineer) of the viewed team can construct.
    let mut tasked: Vec<Entity> = selected_builders
        .iter()
        .filter(|(_, t)| t.0 == view.team)
        .map(|(e, _)| e)
        .collect();
    tasked.sort_by_key(|e| e.to_bits());
    for (interaction, action) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        input.blocked = true;
        match *action {
            Action::Command(command) => {
                if selected_units.is_empty() {
                    continue;
                }
                match command {
                    0 => *pending = PendingOrder::AttackMove,
                    1 => *pending = PendingOrder::Patrol,
                    2 => *pending = PendingOrder::Guard,
                    _ => {
                        *pending = PendingOrder::None;
                        for e in &selected_units {
                            if command == 3 {
                                crate::orders::queue_hold(&mut commands.entity(e));
                            } else {
                                crate::orders::queue_stop(&mut commands.entity(e));
                            }
                        }
                    }
                }
            }
            Action::Build(kind) => {
                if tasked.is_empty() {
                    placement.kind = None;
                    placement.builders.clear();
                    placement.message = "Select a builder first (Commander / Engineer)".into();
                    continue;
                }
                placement.kind = Some(kind);
                placement.builders = tasked.clone();
                placement.message = format!(
                    "Placing {} with {} builder(s): left-click ground, builders march there. Esc / right-click cancels",
                    kind.stats().name,
                    tasked.len()
                );
                *pending = PendingOrder::None;
            }
            Action::Produce(kind) => {
                if let Some((e, _, false)) = selected
                    && let Ok(mut factory) = factories.get_mut(e)
                    && !factory.enqueue(kind)
                {
                    placement.message = "Queue full or unit unavailable for this factory.".into();
                }
            }
            Action::Cancel(index) => {
                if let Some((e, _, false)) = selected
                    && let Ok(mut factory) = factories.get_mut(e)
                {
                    factory.cancel(index);
                }
            }
            Action::CancelSite => {
                if let Some((e, _, true)) = selected {
                    commands.entity(e).despawn();
                    placement.message = "Site cancelled. Spent resources are NOT refunded.".into();
                }
            }
        }
    }
}
/// Build-grid overlay during placement: 2m lines in a patch around the
/// snapped cursor, brighter every 10m. Visual-only, no sim state.
/// G1: magnete Metal→deposito per il ghost: snappa allo spot più vicino
/// entro `SPOT_MAGNET`, altrimenti lascia il punto (la regola rifiuta con
/// messaggio). Puro e deterministico.
fn snap_to_deposit(deposits: &[crate::structures::MetalDeposit], point: Vec3) -> Vec3 {
    let mut best: Option<(f32, Vec3)> = None;
    for dep in deposits {
        let d = dep.pos.xz().distance_squared(point.xz());
        if d <= crate::structures::SPOT_MAGNET * crate::structures::SPOT_MAGNET
            && best.is_none_or(|(bd, _)| d < bd)
        {
            best = Some((d, dep.pos));
        }
    }
    best.map(|(_, p)| point.with_x(p.x).with_z(p.z))
        .unwrap_or(point)
}

fn draw_build_grid(gizmos: &mut Gizmos, center: Vec3) {
    use structures::BUILD_GRID;
    let half = 12.0;
    let y = 0.12;
    let steps = (half * 2.0 / BUILD_GRID).round() as i32;
    let start_x = ((center.x - half) / BUILD_GRID).round() * BUILD_GRID;
    let start_z = ((center.z - half) / BUILD_GRID).round() * BUILD_GRID;
    for i in 0..=steps {
        let x = start_x + i as f32 * BUILD_GRID;
        let z = start_z + i as f32 * BUILD_GRID;
        let major_x = (x % 10.0).abs() < 0.001;
        let major_z = (z % 10.0).abs() < 0.001;
        gizmos.line(
            Vec3::new(x, y, center.z - half),
            Vec3::new(x, y, center.z + half),
            if major_x {
                Color::srgba(0.6, 0.85, 1.0, 0.55)
            } else {
                Color::srgba(0.5, 0.7, 0.9, 0.22)
            },
        );
        gizmos.line(
            Vec3::new(center.x - half, y, z),
            Vec3::new(center.x + half, y, z),
            if major_z {
                Color::srgba(0.6, 0.85, 1.0, 0.55)
            } else {
                Color::srgba(0.5, 0.7, 0.9, 0.22)
            },
        );
    }
}
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn placement_and_rally(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<RtsCamera>>,
    mut placement: ResMut<Placement>,
    input: Res<MapInput>,
    grid: Res<crate::navigation::NavGrid>,
    buildings: Query<
        (&Team, &BuildingKind, &Transform, Has<Construction>, &Health),
        With<Building>,
    >,
    builders: Query<
        (
            Entity,
            &Team,
            &Transform,
            &Builder,
            &CollisionRadius,
            &Health,
        ),
        With<Unit>,
    >,
    units: Query<(&Transform, &CollisionRadius), With<Unit>>,
    interactions: Query<&Interaction, With<BlocksMap>>,
    view: Res<ViewState>,
    deposits: Option<Res<crate::structures::MetalDeposits>>,
    mut gizmos: Gizmos,
) {
    if !window.focused || !input.commands_allowed {
        placement.kind = None;
        placement.builders.clear();
        return;
    }
    if keys.just_pressed(KeyCode::Escape)
        || mouse.just_pressed(MouseButton::Right) && placement.kind.is_some()
    {
        placement.kind = None;
        placement.builders.clear();
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Some(point) = ground_position(camera.0, camera.1, cursor) else {
        return;
    };
    if let Some(kind) = placement.kind {
        // Harness senza StructuresPlugin: depositi vuoti = regola chiusa
        // (niente Metal), mai panic su risorsa assente.
        let empty = crate::structures::MetalDeposits::default();
        let deposits = deposits.as_deref().unwrap_or(&empty);
        // Grid-based: rule, preview, spawn and march all use the snapped
        // point so the site lands exactly where the ghost was.
        // G1: il Metal snappa al deposito (magnete), così il ghost cade
        // sempre sullo spot e la regola sotto conferma.
        let point = structures::snap_to_grid(point);
        let point = if kind == BuildingKind::Metal {
            snap_to_deposit(&deposits.0, point)
        } else {
            point
        };
        let base: Vec<_> = buildings
            .iter()
            .filter(|r| r.4.current > 0.0)
            .map(|(t, k, p, c, _)| (*t, *k, p.translation, c))
            .collect();
        let metals: Vec<Vec3> = base
            .iter()
            .filter(|(_, k, _, _)| *k == BuildingKind::Metal)
            .map(|(_, _, p, _)| *p)
            .collect();
        // Live builders only: the dead neither enable placement nor build.
        let live: Vec<_> = builders
            .iter()
            .filter(|(_, _, _, _, _, h)| h.current > 0.0)
            .map(|(e, t, p, b, r, _)| (e, *t, p.translation, b.radius, b.power, r.0))
            .collect();
        let live_builders: Vec<_> = live.iter().map(|(_, t, p, r, _, _)| (*t, *p, *r)).collect();
        // Tasked builders still alive: only these march on confirm. If they
        // all died mid-preview the placement is dead too — re-task, no
        // silent fallback to other builders.
        let mut tasked: Vec<_> = live
            .iter()
            .filter(|(e, t, _, _, _, _)| t.0 == view.team && placement.builders.contains(e))
            .map(|(e, _, p, r, _, body)| (*e, p.xz().distance(point.xz()), *r, *p, *body))
            .collect();
        tasked.sort_by(|a, b| {
            a.1.total_cmp(&b.1)
                .then_with(|| a.0.to_bits().cmp(&b.0.to_bits()))
        });
        let occupied: Vec<_> = units.iter().map(|(p, r)| (p.translation, r.0)).collect();
        let mut valid = structures::placement_rule(Team(view.team), point, &base, &live_builders)
            .and_then(|()| structures::valid_ground(&grid, kind, point, &occupied))
            .and_then(|()| structures::factory_spawn_ok(&grid, kind, point))
            .and_then(|()| structures::metal_spot_ok(&deposits.0, kind, point, &metals));
        if tasked.is_empty() && valid.is_ok() {
            valid = Err("Tasked builders lost — pick builders and retry");
        }
        placement.message = match (&valid, tasked.first()) {
            (Ok(()), Some((_, dist, radius, _, _))) if *dist <= *radius => {
                format!(
                    "VALID cell ({:.0}, {:.0}): {} tasked builder(s) in range — left-click to place",
                    point.x,
                    point.z,
                    tasked.len()
                )
            }
            (Ok(()), Some((_, dist, radius, _, _))) => format!(
                "VALID cell ({:.0}, {:.0}): {} tasked builder(s), nearest {dist:.0}m away (range {radius:.0}m) — march on confirm",
                point.x,
                point.z,
                tasked.len()
            ),
            (Ok(()), None) => "VALID".into(), // unreachable: empty tasked is INVALID
            (Err(reason), _) => format!("INVALID: {reason}"),
        };
        let color = if valid.is_ok() {
            Color::srgb(0.2, 1.0, 0.4)
        } else {
            Color::srgb(1.0, 0.25, 0.15)
        };
        let s = kind.stats();
        draw_build_grid(&mut gizmos, point);
        // G1: mentre piazzi Metal evidenzia i depositi (oro liberi, rossi
        // occupati, anello largo per il centrale a mult).
        if kind == BuildingKind::Metal {
            for dep in &deposits.0 {
                let taken = metals
                    .iter()
                    .any(|p| p.xz().distance(dep.pos.xz()) <= structures::SPOT_CAPTURE);
                gizmos.circle(
                    Isometry3d::new(
                        dep.pos.with_y(0.15),
                        Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
                    ),
                    if dep.mult > 1.0 { 4.5 } else { 3.5 },
                    if taken {
                        Color::srgb(1.0, 0.25, 0.15)
                    } else {
                        Color::srgb(1.0, 0.8, 0.25)
                    },
                );
            }
        }
        gizmos.cube(
            Transform::from_translation(point.with_y(s.height * 0.5)).with_scale(Vec3::new(
                s.half.x * 2.0,
                s.height,
                s.half.y * 2.0,
            )),
            color,
        );
        for (team, p, radius) in &live_builders {
            if team.0 == view.team {
                gizmos.circle(
                    Isometry3d::new(
                        p.with_y(0.15),
                        Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
                    ),
                    *radius,
                    Color::srgb(0.5, 0.9, 0.4),
                );
            }
        }
        // NOTE: no factory circle — the lab only makes units, placement is
        // builder-driven anywhere on valid ground.
        if mouse.just_pressed(MouseButton::Left)
            && !input.captured
            && !interactions.iter().any(|i| *i != Interaction::None)
            && valid.is_ok()
        {
            // Validation is computed here on the confirming click from live entities.
            // Reachable builder approach on the post-placement grid (not just
            // valid ground): the new footprint itself can seal its stand-off,
            // which would loop MoveTarget/fail forever, so it is rejected
            // with the preview kept armed.
            let approach_ok = tasked.first().is_some_and(|(_, _, _, pos, body)| {
                let probe = grid.cloned_with_obstacle(structures::building_obstacle(kind, point));
                let approach =
                    structures::site_approach(&probe, point, *pos, kind.stats().half, *body);
                probe.find_path_for(*pos, approach, *body).is_some()
            });
            if !approach_ok {
                placement.message =
                    "INVALID: no reachable builder approach — pick a clearer spot".into();
                return;
            }
            let site =
                structures::spawn_building(&mut commands, Team(view.team), kind, point, false);
            // Every tasked builder takes an explicit Build order on the site
            // (multi-selection): resolve marches the out-of-range ones to a
            // stand-off and holds them there. Already-in-range ones just hold.
            // Any later order (manual move away) drops the task and pauses.
            let en_route = tasked.iter().filter(|(_, d, r, _, _)| *d > *r).count();
            for (entity, _, _, _, _) in &tasked {
                crate::orders::queue_build(&mut commands.entity(*entity), site);
            }
            placement.message = if en_route > 0 {
                format!(
                    "Construction placed. {en_route} builder(s) en route, work starts on arrival."
                )
            } else {
                "Construction started. Cancellation gives NO REFUND.".into()
            };
            placement.kind = None;
            placement.builders.clear();
        }
    }
}

/// Rally factory con right-click sul terreno libero: solo se nessuna unità è
/// selezionata (click sul vuoto) e una factory del team visto lo è. Sistema
/// separato da `placement_and_rally`: Bevy accetta max 16 param per sistema.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn set_factory_rally(
    mouse: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<RtsCamera>>,
    grid: Res<crate::navigation::NavGrid>,
    input: Res<MapInput>,
    pending: Res<PendingOrder>,
    view: Res<ViewState>,
    selected_units: Query<(), (With<Unit>, With<Selected>)>,
    mut factories: Query<(&Team, &mut Factory), (With<Selected>, Without<Construction>)>,
) {
    if !window.focused
        || !input.commands_allowed
        || input.blocked
        || *pending != PendingOrder::None
        || !selected_units.is_empty()
        || !mouse.just_pressed(MouseButton::Right)
    {
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let (camera, transform) = *camera;
    let Some(point) = ground_position(camera, transform, cursor) else {
        return;
    };
    if !grid.has_clearance(point) || !grid.is_walkable(point) {
        return;
    }
    for (team, mut factory) in &mut factories {
        if team.0 == view.team {
            factory.rally = Some(point);
        }
    }
}
fn update_text(world: &mut World) {
    let view = *world.resource::<ViewState>();
    let allowed = world.resource::<MapInput>().commands_allowed;
    let selected_units = world
        .query_filtered::<Entity, (With<Selected>, With<Unit>)>()
        .iter(world)
        .count();
    let builder_count = world
        .query_filtered::<&Team, (With<Selected>, With<Builder>)>()
        .iter(world)
        .filter(|t| t.0 == view.team)
        .count();
    let building = world
        .query_filtered::<(Entity, &Team), (With<Selected>, With<Building>)>()
        .iter(world)
        .filter(|(_, t)| t.0 == view.team)
        .map(|(e, _)| e)
        .min_by_key(|e| e.to_bits());
    let inspected = world
        .get_resource::<super::debug::Inspected>()
        .and_then(|i| i.0);
    let entity = building.or(inspected);
    let factory = entity
        .filter(|e| {
            world
                .get::<Team>(*e)
                .is_some_and(|t| view.permits_private_data(t.0))
        })
        .and_then(|e| world.get::<Factory>(e))
        .cloned();
    let own = entity.is_some_and(|e| world.get::<Team>(e).is_some_and(|t| t.0 == view.team));
    let editable = own && allowed && building.is_some();
    let site = entity.is_some_and(|e| world.get::<Construction>(e).is_some());
    let busy = world
        .query_filtered::<&Team, With<Construction>>()
        .iter(world)
        .any(|t| t.0 == view.team);
    let base = if !allowed {
        "READ ONLY | inspect the match".to_string()
    } else if builder_count > 0 {
        format!(
            "CONSTRUCTION | {builder_count} builders{}",
            if busy {
                " | active site already exists"
            } else {
                ""
            }
        )
    } else if factory.is_some() {
        "PRODUCTION | choose a unit".to_string()
    } else if selected_units > 0 {
        "ORDERS | right-click destination / target".to_string()
    } else {
        "Select your Commander to start building.".to_string()
    };
    let pending = *world.resource::<PendingOrder>();
    let placement = world.resource::<Placement>();
    let context = match pending {
        PendingOrder::AttackMove => "ATTACK-MOVE | click destination | Esc cancels".into(),
        PendingOrder::Patrol => "PATROL | click waypoints | Esc finishes".into(),
        PendingOrder::Guard => "GUARD | click an ally | Esc cancels".into(),
        PendingOrder::None if !placement.message.is_empty() && allowed => placement.message.clone(),
        _ if factory.is_some() && editable => {
            "Right-click ground: rally | click queue row: cancel (no refund)".into()
        }
        _ => "F1 help | F3 debug".into(),
    };
    for (mut text, base_marker, context_marker, queue) in world
        .query::<(
            &mut Text,
            Option<&BaseText>,
            Option<&ContextText>,
            Option<&QueueLabel>,
        )>()
        .iter_mut(world)
    {
        if base_marker.is_some() {
            text.set_if_neq(Text::new(base.clone()));
        }
        if context_marker.is_some() {
            text.set_if_neq(Text::new(context.clone()));
        }
        if let Some(q) = queue {
            let value =
                factory
                    .as_ref()
                    .and_then(|f| f.queue.get(q.0))
                    .map_or(String::new(), |job| {
                        format!(
                            "{}. {}{}",
                            q.0 + 1,
                            archetype(job.kind).name,
                            if q.0 == 0 { " | active" } else { "" }
                        )
                    });
            text.set_if_neq(Text::new(value));
        }
    }
    for (mut node, factory_marker, site_marker, queue_marker) in world
        .query::<(
            &mut Node,
            Option<&FactoryPanel>,
            Option<&SiteButton>,
            Option<&QueuePanel>,
        )>()
        .iter_mut(world)
    {
        let show = if factory_marker.is_some() {
            Some(editable && factory.is_some() && !site)
        } else if site_marker.is_some() {
            Some(editable && site)
        } else if queue_marker.is_some() {
            Some(factory.is_some())
        } else {
            None
        };
        if let Some(show) = show {
            node.display = if show { Display::Flex } else { Display::None };
        }
    }
    for (action, mut node, interaction, mut color) in world
        .query::<(&Action, &mut Node, &Interaction, &mut BackgroundColor)>()
        .iter_mut(world)
    {
        let show = match action {
            Action::Build(_) => allowed && builder_count > 0,
            Action::Produce(kind) => {
                editable
                    && factory
                        .as_ref()
                        .is_some_and(|f| archetype(*kind).tier <= f.tier)
            }
            Action::Cancel(i) => factory.as_ref().is_some_and(|f| f.queue.get(*i).is_some()),
            Action::CancelSite => editable && site,
            Action::Command(_) => allowed && selected_units > 0,
        };
        node.display = if show { Display::Flex } else { Display::None };
        color.0 = if *interaction != Interaction::None {
            Color::srgb(0.21, 0.34, 0.39)
        } else {
            super::shell::BUTTON
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        camera::CameraPlugin,
        economy::Economy,
        navigation::{NavGrid, NavigationStats},
        orders::PendingOrder,
        scenario::Scenario,
    };
    use bevy::window::PrimaryWindow;

    /// Regression test B0001: avvia davvero l'intero plugin grafico UI.
    /// Query `&mut Text` ambigue fanno panic al primo frame (solo l'app vera
    /// le eseguiva: `cargo test` da solo non le toccava mai).
    fn ui_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::asset::AssetPlugin::default())
            .add_plugins(bevy::gizmos::GizmoPlugin)
            .add_plugins(bevy::input::InputPlugin)
            .insert_resource(Scenario::Playground)
            .insert_resource(Economy::default())
            .insert_resource(Placement::default())
            .init_resource::<PendingOrder>()
            .init_resource::<NavigationStats>()
            .init_resource::<NavGrid>()
            .init_resource::<MapInput>()
            .init_resource::<crate::spatial::SpatialGrid>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Mesh>()
            .init_asset::<bevy::render::mesh::skinning::SkinnedMeshInverseBindposes>()
            .insert_resource(crate::session::SessionControl::default())
            .add_plugins((
                CameraPlugin,
                crate::ui::UiPlugin,
                IndustryUiPlugin,
                crate::selection::SelectionPlugin,
                crate::orders::OrderPlugin,
            ));
        // Finestra fittizia per i Single<&Window>: basta l'entità.
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.finish();
        app.cleanup();
        // Startup (spawn testi/pulsanti) + frame che eseguono tutti i sistemi:
        // con un conflitto B0001 questo va in panic qui, non in produzione.
        for _ in 0..5 {
            app.update();
        }
        app
    }
    #[test]
    fn industry_plugin_boots_and_ticks_without_access_conflicts() {
        let mut app = ui_app();
        // I pulsanti VIEW/FOG esistono con le label iniziali (team blu, fog ON).
        let view = app.world().resource::<ViewState>();
        assert_eq!(view.team, 0);
        assert!(view.fog_on);
        let labels: Vec<String> = app
            .world_mut()
            .query::<&Text>()
            .iter(app.world())
            .map(|t| t.0.clone())
            .collect();
        assert!(labels.iter().any(|t| t.contains("BLUE")));
        assert!(labels.iter().any(|t| t.contains("Full view")));
    }
    #[test]
    fn spectator_cannot_mutate_units_factories_or_sites_through_ui_or_keys() {
        let mut app = ui_app();
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        let unit = app
            .world_mut()
            .spawn((
                Selected,
                Unit(0),
                Team(0),
                UnitKind::Commander,
                Builder {
                    power: 10.0,
                    radius: 26.0,
                },
                Health {
                    current: 100.0,
                    max: 100.0,
                },
                Transform::default(),
                crate::orders::UnitOrder::Idle,
            ))
            .id();
        let mut factory = Factory::default();
        factory.enqueue(UnitKind::Scout);
        let factory = app
            .world_mut()
            .spawn((
                Selected,
                Building,
                Team(0),
                BuildingKind::Factory,
                Health {
                    current: 100.0,
                    max: 100.0,
                },
                Transform::default(),
                factory,
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::session::SessionControl>()
            .transfer(0, crate::session::Controller::Bot);
        app.update();
        // Deliberately retain Selected to prove permissions are enforced even
        // when a stale selection survives an external input producer.
        app.world_mut().entity_mut(unit).insert(Selected);
        app.world_mut().entity_mut(factory).insert(Selected);
        for action in [
            Action::Build(BuildingKind::Solar),
            Action::Produce(UnitKind::Scout),
            Action::Cancel(0),
            Action::Command(3),
            Action::Command(4),
        ] {
            let button = app
                .world_mut()
                .spawn((Button, action, Interaction::Pressed))
                .id();
            app.update();
            app.world_mut().despawn(button);
        }
        for key in [
            KeyCode::KeyG,
            KeyCode::KeyP,
            KeyCode::KeyT,
            KeyCode::KeyH,
            KeyCode::KeyX,
        ] {
            app.world_mut()
                .write_message(bevy::input::keyboard::KeyboardInput {
                    key_code: key,
                    logical_key: bevy::input::keyboard::Key::Unidentified(
                        bevy::input::keyboard::NativeKey::Unidentified,
                    ),
                    state: bevy::input::ButtonState::Pressed,
                    text: None,
                    repeat: false,
                    window,
                });
            app.update();
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .reset_all();
        }
        assert_eq!(
            *app.world().get::<crate::orders::UnitOrder>(unit).unwrap(),
            crate::orders::UnitOrder::Idle
        );
        assert_eq!(app.world().resource::<PendingOrder>(), &PendingOrder::None);
        assert!(app.world().resource::<Placement>().kind.is_none());
        assert_eq!(app.world().get::<Factory>(factory).unwrap().queue.len(), 1);
        assert!(app.world().get::<Factory>(factory).unwrap().rally.is_none());
    }
    #[test]
    fn ui_pointer_capture_lasts_through_release() {
        let mut app = ui_app();
        let panel = app
            .world_mut()
            .spawn((BlocksMap, Interaction::Hovered))
            .id();
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .write_message(bevy::input::mouse::MouseButtonInput {
                button: MouseButton::Left,
                state: bevy::input::ButtonState::Pressed,
                window,
            });
        app.update();
        assert!(app.world().resource::<MapInput>().blocked);
        *app.world_mut().get_mut::<Interaction>(panel).unwrap() = Interaction::None;
        app.update();
        assert!(app.world().resource::<MapInput>().blocked);
        app.world_mut()
            .write_message(bevy::input::mouse::MouseButtonInput {
                button: MouseButton::Left,
                state: bevy::input::ButtonState::Released,
                window,
            });
        app.update();
        assert!(app.world().resource::<MapInput>().blocked);
        app.update();
        assert!(!app.world().resource::<MapInput>().blocked);
    }
}
