use bevy::{prelude::*, transform::TransformSystems, window::PrimaryWindow};

use crate::{
    camera::RtsCamera,
    orders::PendingOrder,
    picking::ray_box_distance,
    units::{Selectable, SelectionRing, Team, UNIT_HALF_SIZE},
    view::ViewState,
};

pub struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DragSelection>()
            .init_resource::<ViewState>()
            .init_resource::<crate::ui::debug::Inspected>()
            .add_systems(Startup, setup_rectangle)
            .add_systems(
                PostUpdate,
                (select_units, show_rings, update_rectangle)
                    .chain()
                    .after(TransformSystems::Propagate)
                    .in_set(SelectionSystems),
            );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SelectionSystems;

#[derive(Component)]
pub struct Selected;

#[derive(Component)]
struct SelectionRectangle;

#[derive(Resource, Default)]
struct DragSelection {
    start: Option<Vec2>,
    end: Vec2,
    dragging: bool,
}

fn setup_rectangle(mut commands: Commands) {
    commands.spawn((
        SelectionRectangle,
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
            border: UiRect::all(px(1.0)),
            ..default()
        },
        BorderColor::all(Color::srgb(0.3, 1.0, 0.4)),
        BackgroundColor(Color::srgba(0.2, 0.9, 0.4, 0.15)),
        GlobalZIndex(10),
    ));
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn select_units(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<RtsCamera>>,
    units: Query<
        (
            Entity,
            &GlobalTransform,
            &Team,
            Has<Selected>,
            Option<&crate::structures::Footprint>,
            Option<&Visibility>,
        ),
        With<Selectable>,
    >,
    mut drag: ResMut<DragSelection>,
    pending: Res<PendingOrder>,
    input: Res<crate::ui::industry::MapInput>,
    view: Res<ViewState>,
    control: Option<Res<crate::view::SessionControl>>,
    mut inspected: ResMut<crate::ui::debug::Inspected>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        inspected.0 = None;
        for (entity, _, _, selected, _, _) in &units {
            if selected {
                commands.entity(entity).remove::<Selected>();
            }
        }
    }
    if !window.focused || keys.just_pressed(KeyCode::Escape) {
        drag.start = None;
        return;
    }
    if input.blocked {
        drag.start = None;
        return;
    }
    let can_select = |team: u8| {
        control
            .as_deref()
            .map_or(team == view.team, |c| c.can_command(team))
    };
    // While an order is being targeted, left clicks belong to the order
    // system (which runs first): selection stays frozen, no stale drag.
    if *pending != PendingOrder::None {
        drag.start = None;
        return;
    }
    if mouse.just_pressed(MouseButton::Left) {
        drag.start = window.cursor_position();
        drag.dragging = false;
        if let Some(cursor) = drag.start {
            drag.end = cursor;
        }
    }
    let Some(start) = drag.start else {
        return;
    };
    if let Some(cursor) = window.cursor_position() {
        drag.end = cursor;
    }
    drag.dragging |= start.distance(drag.end) >= 5.0;
    let bounds = Rect::from_corners(start, drag.end);

    if !mouse.just_released(MouseButton::Left) {
        return;
    }
    let (camera, camera_transform) = *camera;
    let additive = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let clicked = if drag.dragging {
        None
    } else {
        camera
            .viewport_to_world(camera_transform, drag.end)
            .ok()
            .and_then(|ray| {
                units
                    .iter()
                    // Fog: concealed enemies are not clickable.
                    .filter(|(_, _, _, _, _, vis)| vis.is_none_or(|v| *v != Visibility::Hidden))
                    .filter_map(|(entity, transform, _, _, footprint, _)| {
                        ray_box_distance(
                            &ray,
                            transform.translation(),
                            footprint.map_or(UNIT_HALF_SIZE, |f| f.0),
                        )
                        .map(|distance| (entity, distance))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(entity, _)| entity)
            })
    };
    if !drag.dragging {
        inspected.0 = clicked;
    }
    for (entity, transform, team, selected, footprint, vis) in &units {
        let hit = if drag.dragging {
            can_select(team.0)
                && vis.is_none_or(|v| *v != Visibility::Hidden)
                && footprint.is_none()
                && camera
                    .world_to_viewport(camera_transform, transform.translation())
                    .is_ok_and(|point| bounds.contains(point))
        } else {
            clicked == Some(entity) && can_select(team.0)
        };
        if hit && drag.dragging {
            inspected.0 = Some(entity);
        }
        if hit && !selected {
            commands.entity(entity).insert(Selected);
        } else if !hit && selected && !additive {
            commands.entity(entity).remove::<Selected>();
        }
    }
    drag.start = None;
}

fn update_rectangle(
    drag: Res<DragSelection>,
    mut rectangle: Single<&mut Node, With<SelectionRectangle>>,
) {
    let Some(start) = drag.start.filter(|_| drag.dragging) else {
        rectangle.display = Display::None;
        return;
    };
    let bounds = Rect::from_corners(start, drag.end);
    rectangle.display = Display::Flex;
    rectangle.left = px(bounds.min.x);
    rectangle.top = px(bounds.min.y);
    rectangle.width = px(bounds.width());
    rectangle.height = px(bounds.height());
}

fn show_rings(
    units: Query<Has<Selected>, With<Selectable>>,
    mut rings: Query<(&ChildOf, &mut Visibility), With<SelectionRing>>,
) {
    for (parent, mut visibility) in &mut rings {
        let next = if units.get(parent.parent()).unwrap_or(false) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != next {
            *visibility = next;
        }
    }
}

pub fn cancel_drag(world: &mut World) {
    if let Some(mut drag) = world.get_resource_mut::<DragSelection>() {
        *drag = default();
    }
}
