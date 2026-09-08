//! Persistent native Bevy controls. Pointer capture spans press through release.
use crate::{
    camera::RtsCamera,
    combat::Health,
    economy::{Economy, balance::*},
    orders::{PendingOrder, UnitOrder},
    picking::ground_position,
    production::Factory,
    selection::{Selected, SelectionSystems},
    structures::{self, Building, Construction, Placement},
    units::{Builder, CollisionRadius, Team, Unit, UnitKind, archetype},
    view::ViewState,
};
use bevy::{prelude::*, transform::TransformSystems, window::PrimaryWindow};

#[derive(Resource, Default)]
pub struct MapInput {
    pub blocked: bool,
    captured: bool,
}
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct MapInputSystems;
#[derive(Component)]
pub struct BlocksMap;
pub fn map_input_allowed(input: Res<MapInput>) -> bool {
    !input.blocked
}
#[derive(Component, Clone, Copy)]
enum Action {
    Build(BuildingKind),
    Produce(UnitKind),
    Cancel(usize),
    CancelSite,
    ViewTeam(u8),
    ToggleFog,
}
/// Marker on the three BASE CONSTRUCTION buttons: shown only while a builder
/// of the viewed team (Commander/Engineer) is selected — click builder, menu appears.
#[derive(Component)]
struct BuildButton;
#[derive(Component)]
struct ResourceText;
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
/// Dynamic labels of the VIEW / FOG buttons (text follows `ViewState`).
#[derive(Component)]
struct ViewTeamLabel(u8);
#[derive(Component)]
struct FogLabel;
pub struct IndustryUiPlugin;
impl Plugin for IndustryUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewState>()
            .add_systems(Startup, setup)
            .add_systems(
                PostUpdate,
                (
                    capture_pointer,
                    actions,
                    set_factory_rally,
                    placement_and_rally,
                )
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
fn panel() -> Node {
    Node {
        flex_direction: FlexDirection::Column,
        row_gap: px(5),
        padding: UiRect::all(px(12)),
        ..default()
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
fn setup(mut commands: Commands) {
    commands
        .spawn((
            BlocksMap,
            Interaction::None,
            Node {
                position_type: PositionType::Absolute,
                top: px(0),
                left: px(0),
                right: px(0),
                height: px(62),
                ..panel()
            },
            BackgroundColor(Color::srgb(0.04, 0.07, 0.09)),
        ))
        .with_children(|p| {
            p.spawn((ResourceText, Text::new("Economy starting..."), font(16.0)));
        });
    commands.spawn((BlocksMap, Interaction::None, Node { position_type: PositionType::Absolute, top: px(68), right: px(8), width: px(310), ..panel() }, BackgroundColor(Color::srgba(0.04, 0.07, 0.09, 0.96))))
        .with_children(|p| {
            p.spawn((Text::new("BASE CONSTRUCTION"), font(18.0)));
            p.spawn((BaseText, Text::default(), font(13.0)));
            for kind in BuildingKind::ALL {
                let s = kind.stats();
                button(p, Action::Build(kind), format!("{}   {}M {}E / {:.0}s", s.name, s.cost.resources[0], s.cost.resources[1], s.cost.work / BASE_POWER));
            }
            p.spawn((ContextText, Text::new("Select a building"), font(14.0)));
            p.spawn((SiteButton, Node { display: Display::None, ..panel() })).with_children(|p| button(p, Action::CancelSite, "Cancel site (NO REFUND)".into()));
            p.spawn((FactoryPanel, Node { display: Display::None, ..panel() })).with_children(|p| {
                for kind in UnitKind::PRODUCIBLE { let c = unit_cost(kind); button(p, Action::Produce(kind), format!("+ {}   {}M {}E / {:.0}s", archetype(kind).name, c.resources[0], c.resources[1], c.work / FACTORY_POWER)); }
                p.spawn((Text::new("Queue: click row to cancel.\nSpent resources are NOT refunded.\nRight-click ground: rally point"), font(12.0)));
                for i in 0..MAX_QUEUE {
                    p.spawn((Button, BlocksMap, Action::Cancel(i), Node { display: Display::None, padding: UiRect::axes(px(6), px(3)), ..default() }, BackgroundColor(Color::srgb(0.18, 0.18, 0.21))))
                        .with_children(|p| { p.spawn((QueueLabel(i), Text::default(), font(13.0))); });
                }
            });
            // Team impersonation + fog spectator toggle. Builders of the
            // viewed team task from this same panel (see update_text).
            p.spawn((Text::new("VIEW & FOG"), font(18.0)));
            for team in [0u8, 1u8] {
                p.spawn((
                    Button,
                    BlocksMap,
                    Action::ViewTeam(team),
                    Node {
                        padding: UiRect::all(px(7)),
                        min_height: px(30),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.14, 0.23, 0.29)),
                ))
                .with_children(|p| {
                    p.spawn((ViewTeamLabel(team), Text::default(), font(14.0)));
                });
            }
            p.spawn((
                Button,
                BlocksMap,
                Action::ToggleFog,
                Node {
                    padding: UiRect::all(px(7)),
                    min_height: px(30),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.14, 0.23, 0.29)),
            ))
            .with_children(|p| {
                p.spawn((FogLabel, Text::default(), font(14.0)));
            });
        });
}
fn capture_pointer(
    mouse: Res<ButtonInput<MouseButton>>,
    mut placement: ResMut<Placement>,
    interactions: Query<&Interaction, With<BlocksMap>>,
    mut input: ResMut<MapInput>,
    match_result: Option<Res<crate::game_over::MatchResult>>,
) {
    // Partita finita: solo R (restart) resta attivo — preview scartato e tutto muto.
    if match_result.is_some_and(|r| r.over) {
        placement.kind = None;
        placement.builders.clear();
        input.blocked = true;
        input.captured = false;
        return;
    }
    let pointer = interactions.iter().any(|i| *i != Interaction::None);
    if pointer && (mouse.just_pressed(MouseButton::Left) || mouse.just_pressed(MouseButton::Right))
    {
        input.captured = true;
    }
    input.blocked = pointer || input.captured || placement.kind.is_some();
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
    all_selected: Query<Entity, With<Selected>>,
    mut factories: Query<&mut Factory>,
    mut placement: ResMut<Placement>,
    mut pending: ResMut<PendingOrder>,
    mut input: ResMut<MapInput>,
    mut view: ResMut<ViewState>,
) {
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
            Action::ViewTeam(team) => {
                if view.team != team {
                    view.team = team;
                    // Stale selection belongs to the other team: clear it so
                    // orders can never leak across teams on view switch.
                    for entity in &all_selected {
                        commands.entity(entity).remove::<Selected>();
                    }
                    placement.kind = None;
                    placement.builders.clear();
                    placement.message = format!(
                        "Now playing as {} — select its builders to construct",
                        ViewState::team_name(team)
                    );
                    *pending = PendingOrder::None;
                }
                continue;
            }
            Action::ToggleFog => {
                view.fog_on = !view.fog_on;
                placement.message = if view.fog_on {
                    "Fog ON — honest view of your team".into()
                } else {
                    "Fog OFF — spectator: everything visible".into()
                };
                continue;
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
                {
                    factory.enqueue(kind);
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
    if !window.focused {
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
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_text(
    economy: Res<Economy>,
    placement: Res<Placement>,
    view: Res<ViewState>,
    selected: Query<
        (
            Entity,
            &Team,
            &BuildingKind,
            &Health,
            Option<&Construction>,
            Option<&Factory>,
        ),
        With<Selected>,
    >,
    sites: Query<&Team, With<Construction>>,
    builders: Query<(Entity, &Team, &Builder, &Health), (With<Unit>, With<Builder>)>,
    selected_builders: Query<
        (Entity, &Team, &UnitKind, &Health, &Builder, &UnitOrder),
        (With<Selected>, With<Unit>, With<Builder>),
    >,
    build_targets: Query<&Construction, With<Building>>,
    mut resources: Single<&mut Text, With<ResourceText>>,
    mut base: Single<&mut Text, (With<BaseText>, Without<ResourceText>)>,
    mut context: Single<&mut Text, (With<ContextText>, Without<BaseText>, Without<ResourceText>)>,
    mut labels: Query<
        (&QueueLabel, &mut Text),
        (
            Without<ContextText>,
            Without<BaseText>,
            Without<ResourceText>,
        ),
    >,
    mut factory_panel: Single<&mut Node, With<FactoryPanel>>,
    mut site_button: Single<&mut Node, (With<SiteButton>, Without<FactoryPanel>)>,
    mut queue_buttons: Query<
        (&Action, &mut Node, &Interaction, &mut BackgroundColor),
        (With<Button>, Without<FactoryPanel>, Without<SiteButton>),
    >,
    // VIEW + FOG in un'unica query (Bevy accetta max 16 param per sistema).
    // I Without escludono gli altri possessori di Text: senza, Bevy va in
    // panic B0001 (&mut Text ambiguo) al primo frame grafico.
    mut view_buttons: Query<
        (&mut Text, Option<&ViewTeamLabel>, Option<&FogLabel>),
        (
            Or<(With<ViewTeamLabel>, With<FogLabel>)>,
            Without<QueueLabel>,
            Without<ResourceText>,
            Without<BaseText>,
            Without<ContextText>,
        ),
    >,
) {
    if let Some(account) = economy.0.get(&view.team) {
        let lines: Vec<_> = ["METAL", "ENERGY"]
            .iter()
            .enumerate()
            .map(|(r, name)| {
                let state = if account.stock[r] >= account.capacity[r] - 0.01 {
                    "[STORAGE FULL]"
                } else if account.demand[r] > account.consumption[r] + 0.01 {
                    "[SHORTAGE]"
                } else {
                    ""
                };
                format!(
                    "{name}  {:.0}/{:.0}     +{:.1}/s   -{:.1}/s   net {:+.1}/s  {state}",
                    account.stock[r],
                    account.capacity[r],
                    account.income[r],
                    account.consumption[r],
                    account.income[r] - account.consumption[r]
                )
            })
            .collect();
        resources.set_if_neq(Text::new(lines.join("\n")));
    }
    // Builders work anywhere with valid ground; each trickles power only
    // within its own radius of the site (see economy::site_power). Here show
    // live builder count — per-site speed lives in the selection panel.
    let alive: usize = builders
        .iter()
        .filter(|(_, t, _, h)| t.0 == view.team && h.current > 0.0)
        .count();
    let tasked_selected: usize = selected_builders
        .iter()
        .filter(|(_, t, _, _, _, _)| t.0 == view.team)
        .count();
    let availability = if sites.iter().any(|t| t.0 == view.team) {
        format!("BUSY: one site already active / builders alive: {alive}")
    } else if alive == 0 {
        "STALLED: no builders alive — protect Commander / build Engineer".into()
    } else if tasked_selected == 0 {
        format!("Builders alive: {alive} — select Commander / Engineer to build")
    } else {
        format!("READY: {tasked_selected} builder(s) tasked — pick a structure, click ground")
    };
    base.set_if_neq(Text::new(format!("{availability}\n{}", placement.message)));
    let selected = selected.iter().min_by_key(|row| row.0.to_bits());
    let mut factory = None;
    let mut site_selected = false;
    let description = if let Some((_, team, kind, health, site, industry)) = selected {
        site_selected = site.is_some() && team.0 == view.team;
        factory = industry.filter(|_| site.is_none() && team.0 == view.team);
        let activity = if let Some(site) = site {
            format!(
                "Construction {:.1}% / {:.1} work/s\n{}",
                site.0.fraction() * 100.0,
                site.0.speed,
                site.0.status()
            )
        } else if let Some(f) = industry {
            if f.blocked {
                "OUTPUT BLOCKED: free exit / rally route".into()
            } else if let Some(job) = f.queue.front() {
                format!(
                    "{} {:.1}% / {:.1} work/s\n{}",
                    archetype(job.kind).name,
                    job.project.fraction() * 100.0,
                    job.project.speed,
                    job.project.status()
                )
            } else {
                "Factory idle".into()
            }
        } else {
            format!(
                "Online: +{}M/s +{}E/s",
                kind.stats().income[0],
                kind.stats().income[1]
            )
        };
        format!(
            "\n{} / team {}\nHealth {:.0}/{:.0}\n{activity}",
            kind.stats().name,
            team.0,
            health.current,
            health.max
        )
    } else if let Some((_, team, kind, health, builder, order)) =
        selected_builders.iter().min_by_key(|row| row.0.to_bits())
    {
        // Commander / Engineer selected: guns, build power/radius and the
        // live build task. Right-click a site to (re)task, any other order
        // pauses.
        let guns = if kind.is_commander() {
            "Guns: mitra 20m + missili 34m"
        } else {
            "Unarmed builder"
        };
        let task = match order {
            UnitOrder::Build { site } => match build_targets.get(*site) {
                Ok(construction) => format!(
                    "Building: {:.0}% / {:.1} work/s\n{}",
                    construction.0.fraction() * 100.0,
                    construction.0.speed,
                    construction.0.status()
                ),
                Err(_) => "Build task done".to_string(),
            },
            UnitOrder::Guard { .. } => "Assisting (guarding builder)".to_string(),
            _ => "No build task — place via BASE CONSTRUCTION or right-click a site".to_string(),
        };
        format!(
            "\n{} / team {}\nHealth {:.0}/{:.0}\n{guns}\nBuild: {:.1} work/s / radius {:.0}\n{task}",
            archetype(*kind).name,
            team.0,
            health.current,
            health.max,
            builder.power,
            builder.radius
        )
    } else {
        "\nSelect a building or builder to inspect it.".into()
    };
    context.set_if_neq(Text::new(description));
    factory_panel.display = if factory.is_some() {
        Display::Flex
    } else {
        Display::None
    };
    site_button.display = if site_selected {
        Display::Flex
    } else {
        Display::None
    };
    for (label, mut text) in &mut labels {
        let value = factory
            .and_then(|f| f.queue.get(label.0))
            .map_or(String::new(), |job| {
                format!(
                    "{}: {} {}   [cancel]",
                    label.0 + 1,
                    archetype(job.kind).name,
                    if label.0 == 0 { "ACTIVE" } else { "waiting" }
                )
            });
        text.set_if_neq(Text::new(value));
    }
    for (action, mut node, interaction, mut color) in &mut queue_buttons {
        if let Action::Cancel(index) = action {
            node.display = if factory.is_some_and(|f| f.queue.len() > *index) {
                Display::Flex
            } else {
                Display::None
            };
        }
        // Tier gate: T2 units show only on a selected LabT2 (the enqueue
        // itself also refuses, this just keeps the menu honest).
        if let Action::Produce(kind) = action {
            node.display = if factory.is_some_and(|f| archetype(*kind).tier <= f.tier) {
                Display::Flex
            } else {
                Display::None
            };
        }
        // Contextual builder menu: structure buttons appear only while a
        // builder of the viewed team is selected — click builder, menu appears.
        if matches!(action, Action::Build(_)) {
            node.display = if tasked_selected > 0 {
                Display::Flex
            } else {
                Display::None
            };
        }
        color.0 = match interaction {
            Interaction::Pressed => Color::srgb(0.28, 0.48, 0.52),
            Interaction::Hovered => Color::srgb(0.21, 0.34, 0.39),
            Interaction::None => Color::srgb(0.14, 0.23, 0.29),
        };
    }
    // VIEW / FOG buttons follow ViewState: active team highlighted.
    for (mut text, view_marker, fog_marker) in &mut view_buttons {
        if let Some(marker) = view_marker {
            let active = marker.0 == view.team;
            text.set_if_neq(Text::new(format!(
                "{} {}{}",
                if active { ">" } else { " " },
                ViewState::team_name(marker.0),
                if active { " (you)" } else { "" }
            )));
        } else if fog_marker.is_some() {
            text.set_if_neq(Text::new(format!(
                "Fog: {}",
                if view.fog_on { "ON" } else { "OFF" }
            )));
        }
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
    use bevy::{diagnostic::FrameTimeDiagnosticsPlugin, window::PrimaryWindow};

    /// Regression test B0001: avvia davvero l'intero plugin grafico UI.
    /// Query `&mut Text` ambigue fanno panic al primo frame (solo l'app vera
    /// le eseguiva: `cargo test` da solo non le toccava mai).
    #[test]
    fn industry_plugin_boots_and_ticks_without_access_conflicts() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(FrameTimeDiagnosticsPlugin::default())
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
            .init_asset::<Mesh>()
            .init_asset::<bevy::render::mesh::skinning::SkinnedMeshInverseBindposes>()
            .add_plugins((CameraPlugin, IndustryUiPlugin));
        // Finestra fittizia per i Single<&Window>: basta l'entità.
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.finish();
        app.cleanup();
        // Startup (spawn testi/pulsanti) + frame che eseguono tutti i sistemi:
        // con un conflitto B0001 questo va in panic qui, non in produzione.
        for _ in 0..5 {
            app.update();
        }
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
        assert!(labels.iter().any(|t| t.contains("Fog: ON")));
    }
}
