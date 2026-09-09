//! Shared layout, session controls and presentation-only sampling.
use super::{
    debug::{DebugSettings, DebugTool, Inspected},
    industry::BlocksMap,
};
use crate::{
    combat::Health,
    selection::Selected,
    session::{Controller, SessionControl, ViewState},
    units::{Team, Unit, UnitKind},
};
use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    input::mouse::{MouseScrollUnit, MouseWheel},
    prelude::*,
};

pub const SURFACE: Color = Color::srgb(0.045, 0.07, 0.09);
pub const BUTTON: Color = Color::srgb(0.13, 0.21, 0.26);
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Debug,
    Help,
    Control,
    Spectator,
    Timing,
    Dock,
}
#[derive(Component, Clone, Copy)]
pub enum Label {
    Status,
    Economy,
    DebugButton,
    DebugData,
    Selection,
}
#[derive(Component, Clone, Copy)]
pub enum Action {
    Cancel,
    Debug,
    Help,
    Control,
    Take(u8),
    Release,
    Perspective(Option<u8>),
    FullView,
    Tool(DebugTool),
    Team(Option<u8>),
    SelectedOnly,
    Pause,
    Slower,
    Faster,
    ResetSpeed,
    Base(u8),
    Focus,
}
#[derive(Resource, Default)]
pub struct ShellState {
    pub help: bool,
    pub control_menu: bool,
    pub perspective: Option<u8>,
    pub initialized: bool,
    pub revision: u64,
    pub sampled: f64,
    pub match_started: f32,
}
pub fn font(size: f32) -> TextFont {
    TextFont {
        font_size: FontSize::Px(size),
        ..default()
    }
}
pub fn column() -> Node {
    Node {
        flex_direction: FlexDirection::Column,
        row_gap: px(8),
        min_width: px(0),
        ..default()
    }
}
pub fn button(parent: &mut ChildSpawnerCommands, action: Action, text: &str) {
    parent
        .spawn((
            Button,
            BlocksMap,
            action,
            Node {
                padding: UiRect::axes(px(9), px(7)),
                min_height: px(32),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(BUTTON),
        ))
        .with_children(|p| {
            p.spawn((Text::new(text), font(13.0)));
        });
}
fn label(parent: &mut ChildSpawnerCommands, marker: Label, size: f32) {
    parent.spawn((marker, Text::default(), font(size)));
}
fn row() -> Node {
    Node {
        column_gap: px(6),
        align_items: AlignItems::Center,
        flex_wrap: FlexWrap::Wrap,
        row_gap: px(4),
        ..default()
    }
}
fn panel() -> Node {
    Node {
        padding: UiRect::all(px(10)),
        ..column()
    }
}

pub fn setup(mut commands: Commands, scenario: Res<crate::scenario::Scenario>) {
    if !matches!(*scenario, crate::scenario::Scenario::Playground) {
        return;
    }
    commands.spawn((Node { width: percent(100), height: percent(100), ..column() }, GlobalZIndex(20)))
        .with_children(|root| {
            root.spawn((BlocksMap, Interaction::None, Node { flex_shrink: 0.0, ..panel() }, BackgroundColor(SURFACE))).with_children(|top| {
                top.spawn(row()).with_children(|p| {
                    p.spawn(Node { flex_grow: 1.0, ..default() }).with_children(|p| label(p, Label::Status, 15.0));
                    button(p, Action::Control, "Control");
                    p.spawn((Button, BlocksMap, Action::Debug, Node { padding: UiRect::all(px(8)), ..default() }, BackgroundColor(BUTTON))).with_children(|p| label(p, Label::DebugButton, 13.0));
                    button(p, Action::Help, "Help [F1]");
                });
                label(top, Label::Economy, 14.0);
                top.spawn((Region::Spectator, row())).with_children(|p| {
                    button(p, Action::Perspective(None), "Full view"); button(p, Action::Perspective(Some(0)), "Fog BLUE"); button(p, Action::Perspective(Some(1)), "Fog RED");
                    button(p, Action::Base(0), "Base BLUE"); button(p, Action::Base(1), "Base RED"); button(p, Action::Focus, "Focus entity");
                });
                top.spawn((Region::Timing, row())).with_children(|p| {
                    button(p, Action::Pause, "Pause [Space]"); button(p, Action::Slower, "Slower [-]"); button(p, Action::ResetSpeed, "1x [0]"); button(p, Action::Faster, "Faster [+]");
                });
                top.spawn((Region::Control, Node { display: Display::None, ..row() })).with_children(|p| {
                    button(p, Action::Take(0), "Take control BLUE"); button(p, Action::Take(1), "Take control RED"); button(p, Action::Release, "Leave to bot");
                });
            });
            root.spawn(Node { flex_grow: 1.0, min_height: px(0), column_gap: px(8), ..default() }).with_children(|body| {
                body.spawn(Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), ..column() }).with_children(|main| {
                    main.spawn(Node { flex_grow: 1.0, min_height: px(0), ..default() });
                    main.spawn((Region::Help, BlocksMap, Interaction::None, Node { display: Display::None, max_height: px(220), overflow: Overflow::scroll_y(), ..panel() }, ScrollPosition::default(), BackgroundColor(SURFACE))).with_children(|p| {
                        p.spawn((Text::new("CAMERA  WASD / arrows | Q/E rotate | wheel zoom\nSELECT  click / drag | Shift add | Esc clear / cancel\nORDERS  right click move / attack enemy / guard ally | Shift queue\nG attack-move | P patrol | T guard | H hold | X stop\nBUILD  select builder, choose structure, click ground\nFACTORY  select factory, queue units; right click ground sets rally\nOBSERVER  click visible entities to inspect; Control to take a team\nSpace pause | +/- speed | 0 reset (observer or Simulation debug)\nF1 help | F3 debug | R restart after match\nDebug tools stay active when the panel is closed."), font(13.0)));
                    });
                    main.spawn((Region::Dock, BlocksMap, Interaction::None, Node { height: px(230), flex_shrink: 0.0, ..panel() }, BackgroundColor(SURFACE))).with_children(super::industry::spawn_dock);
                });
                body.spawn((Region::Debug, BlocksMap, Interaction::None, Node { display: Display::None, width: px(320), flex_shrink: 0.0, overflow: Overflow::scroll_y(), ..panel() }, ScrollPosition::default(), BackgroundColor(SURFACE))).with_children(|p| {
                    p.spawn((Text::new("DEBUG TOOLS"), font(16.0)));
                    p.spawn((Text::new("Enable only what you need."), font(13.0)));
                    p.spawn(row()).with_children(|p| { button(p, Action::Team(None), "All"); button(p, Action::Team(Some(0)), "BLUE"); button(p, Action::Team(Some(1)), "RED"); });
                    button(p, Action::SelectedOnly, "[ ] Selected entities only");
                    button(p, Action::FullView, "[ ] Full debug view");
                    for tool in DebugTool::ALL { button(p, Action::Tool(tool), &format!("[ ] {}", tool.label())); }
                    label(p, Label::DebugData, 13.0);
                });
            });
        });
}

/// Clear all gesture state at the same boundary as control/perspective changes.
pub fn clear_transients(world: &mut World) {
    let selected: Vec<Entity> = world
        .query_filtered::<Entity, With<Selected>>()
        .iter(world)
        .collect();
    for entity in selected {
        world.entity_mut(entity).remove::<Selected>();
    }
    if let Some(mut pending) = world.get_resource_mut::<crate::orders::PendingOrder>() {
        *pending = default();
    }
    if let Some(mut placement) = world.get_resource_mut::<crate::structures::Placement>() {
        *placement = default();
    }
    if let Some(mut input) = world.get_resource_mut::<super::industry::MapInput>() {
        input.cancel_gesture();
    }
    if let Some(mut inspected) = world.get_resource_mut::<Inspected>() {
        inspected.0 = None;
    }
    crate::selection::cancel_drag(world);
}

pub fn actions(world: &mut World) {
    if !world.contains_resource::<SessionControl>() {
        return;
    }
    let mut actions: Vec<Action> = world
        .query_filtered::<(&Interaction, &Action), (Changed<Interaction>, With<Button>)>()
        .iter(world)
        .filter(|(i, _)| **i == Interaction::Pressed)
        .map(|(_, a)| *a)
        .collect();
    let focused = world
        .query_filtered::<&Window, With<bevy::window::PrimaryWindow>>()
        .iter(world)
        .next()
        .is_some_and(|w| w.focused);
    if !focused {
        return;
    }
    if let Some(keys) = world.get_resource::<ButtonInput<KeyCode>>() {
        if keys.just_pressed(KeyCode::Escape) {
            actions.push(Action::Cancel);
        }
        if keys.just_pressed(KeyCode::F1) {
            actions.push(Action::Help);
        }
        if keys.just_pressed(KeyCode::F3) {
            actions.push(Action::Debug);
        }
    }
    if !world.resource::<ShellState>().initialized {
        let view = *world.resource::<ViewState>();
        let mut shell = world.resource_mut::<ShellState>();
        shell.perspective = view.fog_on.then_some(view.team);
        shell.initialized = true;
    }
    let over = world
        .get_resource::<crate::game_over::MatchResult>()
        .is_some_and(|r| r.over);
    for action in actions {
        match action {
            Action::Cancel => clear_transients(world),
            Action::Debug => world.resource_mut::<DebugSettings>().open ^= true,
            Action::Help => world.resource_mut::<ShellState>().help ^= true,
            Action::Control => world.resource_mut::<ShellState>().control_menu ^= true,
            Action::Tool(tool) => world.resource_mut::<DebugSettings>().toggle(tool),
            Action::Team(team) => world.resource_mut::<DebugSettings>().team = team,
            Action::SelectedOnly => world.resource_mut::<DebugSettings>().selected_only ^= true,
            Action::FullView => {
                world.resource_mut::<DebugSettings>().full_view ^= true;
                clear_transients(world);
            }
            Action::Perspective(team)
                if world.resource::<SessionControl>().player_team().is_none() =>
            {
                world.resource_mut::<ShellState>().perspective = team;
                clear_transients(world);
            }
            Action::Take(team) if !over => {
                world
                    .resource_mut::<SessionControl>()
                    .transfer(team, Controller::Human);
            }
            Action::Release if !over => {
                if let Some(team) = world.resource::<SessionControl>().player_team() {
                    world
                        .resource_mut::<SessionControl>()
                        .transfer(team, Controller::Bot);
                }
            }
            Action::Pause | Action::Slower | Action::Faster | Action::ResetSpeed => {
                if let Some(mut speed) = world.get_resource_mut::<crate::ai::debug::AiSpeed>() {
                    match action {
                        Action::Pause => speed.toggle_pause(),
                        Action::Slower => speed.step_down(),
                        Action::Faster => speed.step_up(),
                        _ => speed.reset(),
                    }
                    let value = speed.speed();
                    world
                        .resource_mut::<Time<Virtual>>()
                        .set_relative_speed(value);
                }
            }
            Action::Base(team) => {
                let point = world
                    .resource::<crate::scenario::Scenario>()
                    .center(team as usize);
                world.resource_mut::<crate::camera::CameraFocus>().0 = Some(point);
            }
            Action::Focus => {
                if let Some(entity) = world.resource::<Inspected>().0
                    && let Some(t) = world.get::<Transform>(entity)
                {
                    let point = t.translation;
                    world.resource_mut::<crate::camera::CameraFocus>().0 = Some(point);
                }
            }
            _ => {}
        }
    }
    if over {
        *world.resource_mut::<crate::orders::PendingOrder>() = default();
    }
    let control = world.resource::<SessionControl>().clone();
    if world.resource::<ShellState>().revision != control.revision {
        clear_transients(world);
        world.resource_mut::<ShellState>().revision = control.revision;
        world.resource_mut::<ShellState>().control_menu = false;
        world.resource_mut::<ShellState>().perspective = control.player_team();
    }
    let debug = world.resource::<DebugSettings>().clone();
    let perspective = control
        .player_team()
        .or(world.resource::<ShellState>().perspective);
    let next = ViewState {
        team: perspective.unwrap_or(0),
        fog_on: !debug.full_view && perspective.is_some(),
    };
    if *world.resource::<ViewState>() != next {
        *world.resource_mut::<ViewState>() = next;
    }
    if let Some(mut ai_debug) = world.get_resource_mut::<crate::ai::debug::AiDebugState>() {
        ai_debug.enabled = debug.on(DebugTool::Decisions);
        if !ai_debug.enabled {
            ai_debug.last_intents.clear();
        }
    }
    let inspected = world.resource::<Inspected>().0;
    if inspected.is_some_and(|e| !entity_visible(world, e, &next)) {
        world.resource_mut::<Inspected>().0 = None;
    }
    let shell = world.resource::<ShellState>();
    let help = shell.help;
    let menu = shell.control_menu;
    for (region, mut node) in world.query::<(&Region, &mut Node)>().iter_mut(world) {
        let show = match region {
            Region::Debug => debug.open,
            Region::Help => help,
            Region::Control => menu,
            Region::Spectator => control.player_team().is_none(),
            Region::Timing => control.player_team().is_none() || debug.on(DebugTool::Simulation),
            Region::Dock => true,
        };
        if *region == Region::Dock {
            node.height = px(if control.player_team().is_none() {
                160
            } else {
                230
            });
        }
        node.set_if_neq(Node {
            display: if show { Display::Flex } else { Display::None },
            ..node.clone()
        });
    }
    let speed = world
        .get_resource::<crate::ai::debug::AiSpeed>()
        .map(|s| s.paused)
        .unwrap_or(false);
    let button_rows: Vec<_> = world
        .query::<(Entity, &Action, &Children, &Interaction)>()
        .iter(world)
        .map(|(e, a, c, i)| (e, *a, c.iter().collect::<Vec<_>>(), *i))
        .collect();
    for (entity, action, children, interaction) in button_rows {
        let (text, active) = match action {
            Action::Tool(t) => (
                Some(format!(
                    "[{}] {}",
                    if debug.on(t) { "x" } else { " " },
                    t.label()
                )),
                debug.on(t),
            ),
            Action::FullView => (
                Some(format!(
                    "[{}] Full debug view",
                    if debug.full_view { "x" } else { " " }
                )),
                debug.full_view,
            ),
            Action::SelectedOnly => (
                Some(format!(
                    "[{}] Selected entities only",
                    if debug.selected_only { "x" } else { " " }
                )),
                debug.selected_only,
            ),
            Action::Team(t) => (None, debug.team == t),
            Action::Perspective(t) => (None, perspective == t),
            Action::Pause => (
                Some(
                    if speed {
                        "Resume [Space]"
                    } else {
                        "Pause [Space]"
                    }
                    .into(),
                ),
                speed,
            ),
            _ => (None, false),
        };
        if let Some(text) = text {
            for child in children {
                if let Some(mut t) = world.get_mut::<Text>(child) {
                    t.set_if_neq(Text::new(text.clone()));
                }
            }
        }
        if let Some(mut color) = world.get_mut::<BackgroundColor>(entity) {
            color.0 = if active {
                Color::srgb(0.18, 0.38, 0.43)
            } else if interaction != Interaction::None {
                Color::srgb(0.22, 0.32, 0.39)
            } else {
                BUTTON
            };
        }
    }
}

pub fn entity_visible(world: &World, entity: Entity, view: &ViewState) -> bool {
    let Some(team) = world.get::<Team>(entity) else {
        return false;
    };
    let Some(transform) = world.get::<Transform>(entity) else {
        return false;
    };
    if world
        .get::<Health>(entity)
        .is_some_and(|h| h.current <= 0.0)
    {
        return false;
    }
    view.permits_private_data(team.0)
        || world
            .get_resource::<crate::fog::VisibilityMap>()
            .is_some_and(|m| m.visible(view.team, transform.translation))
}

pub fn scroll_panels(
    mut wheel: MessageReader<MouseWheel>,
    mut panels: Query<(Entity, &ComputedNode, &mut ScrollPosition)>,
    hovered: Query<(Entity, &Interaction)>,
    parents: Query<&ChildOf>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let amount: f32 = wheel
        .read()
        .map(|e| {
            e.y * if e.unit == MouseScrollUnit::Line {
                28.0
            } else {
                1.0
            }
        })
        .sum();
    let key_amount = if keys.just_pressed(KeyCode::PageDown) {
        150.0
    } else if keys.just_pressed(KeyCode::PageUp) {
        -150.0
    } else {
        0.0
    };
    if amount == 0.0 && key_amount == 0.0 {
        return;
    }
    // Buttons stop UI hit propagation. Walk from the hit button to its
    // nearest scroll container, rather than expecting the parent to hover.
    let mut targets = std::collections::HashSet::new();
    for (mut entity, interaction) in &hovered {
        if *interaction == Interaction::None {
            continue;
        }
        loop {
            if panels.contains(entity) {
                targets.insert(entity);
                break;
            }
            let Ok(parent) = parents.get(entity) else {
                break;
            };
            entity = parent.parent();
        }
    }
    for (entity, node, mut position) in &mut panels {
        if !targets.contains(&entity) {
            continue;
        }
        let max = ((node.content_size().y - node.size().y) * node.inverse_scale_factor).max(0.0);
        position.0.y = (position.0.y - amount + key_amount).clamp(0.0, max);
    }
}

fn economy_line(world: &World, team: u8, detailed: bool) -> String {
    let fallback = crate::economy::Account::default();
    let account = world
        .get_resource::<crate::economy::Economy>()
        .and_then(|e| e.0.get(&team))
        .unwrap_or(&fallback);
    ["Metal", "Energy"]
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let state = if account.demand[i] > account.consumption[i] + 0.01 {
                " SHORTAGE"
            } else if account.stock[i] >= account.capacity[i] - 0.01 {
                " FULL"
            } else {
                ""
            };
            let basic = format!(
                "{name} {:.0}/{:.0}  {:+.1}/s{state}",
                account.stock[i],
                account.capacity[i],
                account.income[i] - account.consumption[i]
            );
            if detailed {
                format!(
                    "{basic}\n  income {:.1} / used {:.1} / demand {:.1}",
                    account.income[i], account.consumption[i], account.demand[i]
                )
            } else {
                basic
            }
        })
        .collect::<Vec<_>>()
        .join(if detailed { "\n" } else { "    |    " })
}

pub fn describe_entity(world: &World, entity: Entity, view: &ViewState, technical: bool) -> String {
    if !entity_visible(world, entity, view) {
        return "No visible entity selected.".into();
    }
    let team = world.get::<Team>(entity).unwrap().0;
    let name = world
        .get::<UnitKind>(entity)
        .map(|k| crate::units::archetype(*k).name)
        .or_else(|| {
            world
                .get::<crate::economy::balance::BuildingKind>(entity)
                .map(|k| k.stats().name)
        })
        .unwrap_or("Entity");
    let mut out = format!("{} | {name}", ViewState::team_name(team));
    if let Some(hp) = world.get::<Health>(entity) {
        out += &format!("\nHealth {:.0}/{:.0}", hp.current, hp.max);
    }
    if !view.permits_private_data(team) {
        return out + "\nVisible enemy | private data hidden";
    }
    if let Some(order) = world.get::<crate::orders::UnitOrder>(entity) {
        out += &format!("\n{}", order_name(order));
        if technical {
            out += &format!("\nOrder: {order:?}");
        }
    }
    if let Some(builder) = world.get::<crate::units::Builder>(entity) {
        out += &format!(
            "\nBuilder {:.0} work/s | range {:.0}m",
            builder.power, builder.radius
        );
    }
    if let Some(site) = world.get::<crate::structures::Construction>(entity) {
        out += &format!(
            "\nBuilding {:.0}% | {:.1} work/s\n{}",
            site.0.fraction() * 100.0,
            site.0.speed,
            site.0.status()
        );
    }
    if let Some(f) = world.get::<crate::production::Factory>(entity) {
        out += &format!("\nFactory T{} | queue {}/12", f.tier, f.queue.len());
        if f.blocked {
            out += "\nOUTPUT BLOCKED: clear exit / rally";
        }
        if let Some(job) = f.queue.front() {
            out += &format!(
                "\n{} {:.0}% | {}",
                crate::units::archetype(job.kind).name,
                job.project.fraction() * 100.0,
                job.project.status()
            );
        }
        if technical {
            out += &format!("\nRally: {:?}", f.rally);
        }
    }
    if technical {
        out += &format!("\nEntity: {entity:?}");
        if let Some(t) = world.get::<Transform>(entity) {
            out += &format!("\nPosition: {:.1}, {:.1}", t.translation.x, t.translation.z);
        }
        if let Some(queue) = world.get::<crate::orders::UnitOrderQueue>(entity) {
            out += &format!("\nQueued orders: {}\n{:?}", queue.0.len(), queue.0);
        }
        if let Some(target) = world.get::<crate::combat::AttackTarget>(entity) {
            out += &if entity_visible(world, target.0, view) {
                format!("\nAttack target: {:?}", target.0)
            } else {
                "\nAttack target: not visible".into()
            };
        }
        if let Some(route) = world.get::<crate::navigation::Route>(entity) {
            out += &format!(
                "\nRemaining route: {} points\n{:?}",
                route.points.len().saturating_sub(route.next),
                &route.points[route.next.min(route.points.len())..]
            );
        } else {
            out += "\nNo active route";
        }
    }
    out
}
fn order_name(order: &crate::orders::UnitOrder) -> &'static str {
    use crate::orders::UnitOrder::*;
    match order {
        Idle => "Idle",
        Move { .. } => "Moving",
        Attack { .. } => "Attacking",
        AttackMove { .. } => "Attack-move",
        HoldPosition => "Holding position",
        Patrol { .. } => "Patrolling",
        Guard { .. } => "Guarding",
        Build { .. } => "Building",
    }
}

pub fn sample(world: &mut World) {
    if !world.contains_resource::<SessionControl>() {
        return;
    }
    let now = world.resource::<Time<Real>>().elapsed_secs_f64();
    if now - world.resource::<ShellState>().sampled < 0.25 {
        return;
    }
    world.resource_mut::<ShellState>().sampled = now;
    let view = *world.resource::<ViewState>();
    let control = world.resource::<SessionControl>().clone();
    let settings = world.resource::<DebugSettings>().clone();
    let inspected = world.resource::<Inspected>().0;
    let clock = crate::ai::debug::format_game_clock(
        world.resource::<Time<Virtual>>().elapsed_secs()
            - world.resource::<ShellState>().match_started,
    );
    let speed = world
        .get_resource::<crate::ai::debug::AiSpeed>()
        .map_or(String::new(), |s| {
            if s.paused {
                "PAUSED".into()
            } else {
                format!("{:.2}x", s.speed())
            }
        });
    let role = control.player_team().map_or("SPECTATOR".into(), |team| {
        format!("PLAYING {}", ViewState::team_name(team))
    });
    let status = format!(
        "{role}   {clock}   {speed}{}",
        if settings.full_view {
            "   FULL DEBUG VIEW"
        } else {
            ""
        }
    );
    let economy = if let Some(team) = control.player_team() {
        economy_line(world, team, false)
    } else {
        (0..2)
            .map(|team| {
                if !view.permits_private_data(team) {
                    return format!(
                        "{} | fog perspective: private data hidden",
                        ViewState::team_name(team)
                    );
                }
                let count = world
                    .query::<(&Team, &Health, Option<&Unit>)>()
                    .iter(world)
                    .filter(|(t, h, u)| t.0 == team && h.current > 0.0 && u.is_some())
                    .count();
                let owner = match control.teams[team as usize] {
                    Controller::Human => "Human",
                    Controller::Bot => "Bot",
                    Controller::Inactive => "Inactive",
                };
                format!(
                    "{} | {owner} | {count} units | {}",
                    ViewState::team_name(team),
                    economy_line(world, team, false)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let count = world
        .query_filtered::<Entity, With<Selected>>()
        .iter(world)
        .count();
    let selection = inspected.map_or_else(
        || {
            if control.player_team().is_some() {
                "Select a unit or building.\nDrag to select your army.".into()
            } else {
                "Click a visible entity to inspect it.".into()
            }
        },
        |e| describe_entity(world, e, &view, false),
    );
    let selection = if count > 1 {
        format!("{count} units selected\n{selection}")
    } else {
        selection
    };
    let mut diagnostics = Vec::new();
    if settings.on(DebugTool::Performance) {
        let fps = world
            .get_resource::<DiagnosticsStore>()
            .and_then(|d| d.get(&FrameTimeDiagnosticsPlugin::FPS))
            .and_then(|d| d.smoothed())
            .unwrap_or(0.0);
        let frame = world
            .get_resource::<DiagnosticsStore>()
            .and_then(|d| d.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME))
            .and_then(|d| d.smoothed())
            .unwrap_or(0.0);
        diagnostics.push(format!("PERFORMANCE\n{fps:.0} FPS | {frame:.2} ms/frame"));
    }
    if settings.on(DebugTool::Simulation) {
        let mut count = 0;
        let mut engaging = 0;
        for (e, t, target) in world
            .query_filtered::<(Entity, &Team, Option<&crate::combat::AttackTarget>), With<Unit>>()
            .iter(world)
        {
            if settings.allows(&view, t.0)
                && (!settings.selected_only
                    || inspected == Some(e)
                    || world.get::<Selected>(e).is_some())
            {
                count += 1;
                engaging += usize::from(target.is_some());
            }
        }
        // Global projectile count is private under a team perspective.
        let projectiles = if !view.fog_on && settings.team.is_none() && !settings.selected_only {
            world
                .query_filtered::<Entity, With<crate::combat::Projectile>>()
                .iter(world)
                .count()
                .to_string()
        } else {
            "global count hidden by filter".into()
        };
        diagnostics.push(format!(
            "SIMULATION\nUnits {count} | engaging {engaging}\nProjectiles: {projectiles}"
        ));
    }
    if settings.on(DebugTool::Navigation) {
        let value = if !view.fog_on && settings.team.is_none() && !settings.selected_only {
            world
                .get_resource::<crate::navigation::NavigationStats>()
                .map_or("Navigation unavailable".into(), |n| {
                    format!(
                        "Planned {} | failed {}\nLast batch {} | {:.2} ms",
                        n.planned, n.failed, n.last_planned, n.last_ms
                    )
                })
        } else {
            "Global totals require full view and All teams.\nUse Selected routes for local inspection.".into()
        };
        diagnostics.push(format!("NAVIGATION\n{value}"));
    }
    if settings.on(DebugTool::Economy) {
        for team in 0..2 {
            if settings.allows(&view, team) {
                diagnostics.push(format!(
                    "ECONOMY {}\n{}",
                    ViewState::team_name(team),
                    economy_line(world, team, true)
                ));
            }
        }
    }
    if settings.on(DebugTool::Decisions) || settings.on(DebugTool::Knowledge) {
        for team in 0..2 {
            if !settings.allows(&view, team) {
                continue;
            }
            if settings.on(DebugTool::Decisions) {
                let owner = control.teams[team as usize];
                let mut value = format!("AI {} | {owner:?}", ViewState::team_name(team));
                if let Some(config) = world.get_resource::<crate::ai::AiConfig>()
                    && let Some(brain) = config.teams.iter().find(|b| b.team == team)
                {
                    value += &format!(" | {}", brain.personality.name);
                }
                if owner != Controller::Bot {
                    value += "\nBot is not commanding this team.";
                }
                if let Some(state) = world.get_resource::<crate::ai::AiState>()
                    && let Some(stats) = state.per_team.get(&team)
                {
                    value += &format!(
                        "\nOrders {} | waves {} | micro {}",
                        stats.orders_issued, stats.waves_launched, stats.micro_orders
                    );
                }
                if let Some(debug) = world.get_resource::<crate::ai::debug::AiDebugState>() {
                    for intent in debug
                        .last_intents
                        .iter()
                        .filter(|s| {
                            s.starts_with(&format!("M{team}:"))
                                || s.starts_with(&format!("m{team}:"))
                        })
                        .take(8)
                    {
                        value += &format!("\n{intent}");
                    }
                }
                diagnostics.push(value);
            }
            if settings.on(DebugTool::Knowledge) {
                let value = world
                    .get_resource::<crate::ai::AiSnapshots>()
                    .and_then(|s| s.0.get(&team))
                    .map_or("Snapshot unavailable".into(), |s| {
                        format!(
                            "Visible {} | remembered {}\nExplored {:.1}% | tick {}",
                            s.visible_enemies.len(),
                            s.memory.len(),
                            s.explored_pct * 100.0,
                            s.tick
                        )
                    });
                diagnostics.push(format!("KNOWLEDGE {}\n{value}", ViewState::team_name(team)));
            }
        }
    }
    if settings.on(DebugTool::Inspector) {
        let text = inspected
            .filter(|e| {
                world
                    .get::<Team>(*e)
                    .is_some_and(|t| settings.allows(&view, t.0))
            })
            .map_or(
                "Click an entity allowed by the current filters.".into(),
                |e| describe_entity(world, e, &view, true),
            );
        diagnostics.push(format!("INSPECTOR\n{text}"));
    }
    let debug_data = if settings.count() == 0 {
        "No tools enabled.".into()
    } else if diagnostics.is_empty() {
        "Overlays enabled.\nNo data for this perspective/filter.".into()
    } else {
        diagnostics.join("\n\n")
    };
    let debug_button = format!("Debug ({}) [F3]", settings.count());
    for (label, mut text) in world.query::<(&Label, &mut Text)>().iter_mut(world) {
        let value = match label {
            Label::Status => &status,
            Label::Economy => &economy,
            Label::DebugButton => &debug_button,
            Label::DebugData => &debug_data,
            Label::Selection => &selection,
        };
        text.set_if_neq(Text::new(value.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspector_cannot_expose_hidden_enemy_and_override_is_reversible() {
        let mut world = World::new();
        let enemy = world
            .spawn((
                Team(1),
                UnitKind::Commander,
                Transform::default(),
                Health {
                    current: 100.0,
                    max: 100.0,
                },
                crate::orders::UnitOrder::HoldPosition,
            ))
            .id();
        let honest = ViewState::default();
        assert!(!entity_visible(&world, enemy, &honest));
        assert!(!describe_entity(&world, enemy, &honest, true).contains("HoldPosition"));
        let full = ViewState {
            fog_on: false,
            ..honest
        };
        assert!(describe_entity(&world, enemy, &full, true).contains("HoldPosition"));
        assert!(!describe_entity(&world, enemy, &honest, true).contains("HoldPosition"));
        world.despawn(enemy);
        assert!(!entity_visible(&world, enemy, &full));
    }
}
