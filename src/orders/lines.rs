//! Beyond-All-Reason style order graphics for selected units only.
//!
//! Three layers:
//!
//! - Order line: straight unit-to-destination segment in the order color,
//!   alive while the unit stays selected with an order.
//! - Destination marker: flat ring on the goal, same color.
//! - Route flash: the actually planned polyline, spawned on every fresh
//!   `Route` (initial order and any replan) with a fade-out timer.
//!
//! All entities are visual-only snapshots: the simulation never reads them
//! back, so determinism is untouched. Selection-bounded counts keep the
//! cost negligible; headless runs never load `OrderPlugin`.

use bevy::prelude::*;

use super::{UnitOrder, order_color};
use crate::{
    movement::{MoveTarget, yaw_toward},
    navigation::Route,
    selection::Selected,
    spatial::SpatialGrid,
    units::{HealthBarBg, HealthBarFg, Turret, Unit},
};

pub const FLASH_TIME: f32 = 1.0;
pub const LINE_Y: f32 = 0.08;
pub const MARKER_Y: f32 = 0.06;
pub const LINE_THICKNESS: f32 = 0.14;

pub struct LinesPlugin;

impl Plugin for LinesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LineAssets>()
            .add_systems(Startup, setup_line_assets)
            .add_systems(
                PostUpdate,
                (flash_planned_routes, update_order_graphics).chain(),
            );
    }
}

#[derive(Resource, Default)]
struct LineAssets {
    segment: Handle<Mesh>,
    marker: Handle<Mesh>,
    order_material: [Handle<StandardMaterial>; 7],
    flash_material: [Handle<StandardMaterial>; 7],
}

/// Which order colour slot an entity uses: Move, Attack, Hold, Idle, Patrol,
/// Guard, Build.
pub fn order_slot(order: &UnitOrder) -> usize {
    match order {
        UnitOrder::Move { .. } => 0,
        UnitOrder::AttackMove { .. } | UnitOrder::Attack { .. } => 1,
        UnitOrder::HoldPosition => 2,
        UnitOrder::Idle => 3,
        UnitOrder::Patrol { .. } => 4,
        UnitOrder::Guard { .. } => 5,
        UnitOrder::Build { .. } => 6,
    }
}

/// One visual entity per drawn segment/marker, keyed by owning unit.
/// A single component type keeps all visual writes in one query, so no
/// two systems ever need overlapping mutable access to transforms.
#[derive(Component)]
struct OrderViz {
    unit: Entity,
    kind: VizKind,
}

enum VizKind {
    Line { order: UnitOrder },
    Marker { order: UnitOrder },
    Flash { remaining: f32 },
}

/// World transform stretching a unit box between two ground points.
pub fn segment_transform(from: Vec3, to: Vec3, thickness: f32, y: f32) -> Transform {
    let offset = to - from;
    let length = offset.xz().length().max(0.001);
    Transform {
        translation: Vec3::new((from.x + to.x) * 0.5, y, (from.z + to.z) * 0.5),
        rotation: Quat::from_rotation_y(yaw_toward(offset)),
        scale: Vec3::new(thickness, thickness, length),
    }
}

fn setup_line_assets(
    mut assets: ResMut<LineAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    assets.segment = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    assets.marker = meshes.add(Annulus::new(1.0, 1.25));
    let orders = [
        UnitOrder::Move {
            destination: Vec3::ZERO,
        },
        UnitOrder::AttackMove {
            destination: Vec3::ZERO,
        },
        UnitOrder::HoldPosition,
        UnitOrder::Idle,
        UnitOrder::Patrol {
            points: Vec::new(),
            next: 0,
        },
        UnitOrder::Guard {
            target: Entity::from_bits(9),
        },
        UnitOrder::Build {
            site: Entity::from_bits(9),
        },
    ];
    for (slot, order) in orders.into_iter().enumerate() {
        assets.order_material[slot] = materials.add(StandardMaterial {
            base_color: order_color(&order),
            unlit: true,
            ..default()
        });
        assets.flash_material[slot] = materials.add(StandardMaterial {
            base_color: order_color(&order).with_alpha(0.9),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        });
    }
}

/// Snapshot the freshly planned remaining route as fading segments.
/// Runs on `Added<Route>` for selected units, so both the initial plan
/// and every replan flash exactly once. Each segment owns a cloned
/// transparent material so fade-out never touches shared assets.
#[allow(clippy::type_complexity)]
fn flash_planned_routes(
    mut commands: Commands,
    assets: Res<LineAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    grid: Res<SpatialGrid>,
    units: Query<(Entity, &Route, &UnitOrder), (With<Selected>, Added<Route>)>,
) {
    for (entity, route, order) in &units {
        let Some(origin) = grid.position(entity) else {
            continue;
        };
        let slot = order_slot(order);
        let template = materials
            .get(&assets.flash_material[slot])
            .cloned()
            .unwrap_or(StandardMaterial {
                base_color: order_color(order).with_alpha(0.9),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                ..default()
            });
        let mut points = Vec::with_capacity(route.points.len() - route.next + 1);
        points.push(origin.with_y(0.0));
        points.extend(route.points[route.next..].iter().copied());
        for leg in points.windows(2) {
            if leg[0].distance_squared(leg[1]) <= f32::EPSILON {
                continue;
            }
            commands.spawn((
                OrderViz {
                    unit: entity,
                    kind: VizKind::Flash {
                        remaining: FLASH_TIME,
                    },
                },
                Mesh3d(assets.segment.clone()),
                MeshMaterial3d(materials.add(template.clone())),
                segment_transform(leg[0], leg[1], LINE_THICKNESS, LINE_Y + 0.02),
            ));
        }
    }
}

/// Persistent order lines + destination markers for selected units, plus
/// fade-out ticking for flashes. Single writer of viz transforms: the only
/// mutable `Transform` access over entities that are provably disjoint from
/// bars, rings and turrets (each carries a marker this query excludes).
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_order_graphics(
    mut commands: Commands,
    time: Res<Time>,
    assets: Res<LineAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    grid: Res<SpatialGrid>,
    units: Query<(Entity, Option<&MoveTarget>, &UnitOrder), With<Selected>>,
    sites: Query<(Entity, &Transform), With<crate::structures::Building>>,
    mut viz: Query<
        (
            Entity,
            &mut OrderViz,
            &mut Transform,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        (
            Without<Unit>,
            Without<crate::structures::Building>,
            Without<HealthBarBg>,
            Without<HealthBarFg>,
            Without<Turret>,
        ),
    >,
) {
    let dt = time.delta_secs();
    let mut live = std::collections::HashSet::new();
    for (entity, target, order) in &units {
        let Some(origin) = grid.position(entity) else {
            continue;
        };
        // Guards holding near their ward carry no MoveTarget: draw the
        // ward link instead so the order stays visible. Same for builders
        // holding at their site.
        let goal = target.map(|target| target.0).or_else(|| match order {
            UnitOrder::Guard { target } => grid.position(*target),
            UnitOrder::Build { site } => sites
                .get(*site)
                .ok()
                .map(|(_, transform)| transform.translation),
            _ => None,
        });
        let Some(goal) = goal else {
            continue;
        };
        let slot = order_slot(order);
        live.insert((entity, 0u8));
        live.insert((entity, 1u8));
        let mut has_line = false;
        let mut has_marker = false;
        for (_, mut viz, mut viz_transform, mut material) in viz.iter_mut() {
            if viz.unit != entity {
                continue;
            }
            match &mut viz.kind {
                VizKind::Line { order: drawn } => {
                    has_line = true;
                    if *drawn != *order {
                        *drawn = order.clone();
                        material.0 = assets.order_material[slot].clone();
                    }
                    *viz_transform = segment_transform(origin, goal, LINE_THICKNESS, LINE_Y);
                }
                VizKind::Marker { order: drawn } => {
                    has_marker = true;
                    if *drawn != *order {
                        *drawn = order.clone();
                        material.0 = assets.order_material[slot].clone();
                    }
                    viz_transform.translation = goal.with_y(MARKER_Y);
                }
                VizKind::Flash { .. } => {}
            }
        }
        if !has_line {
            commands.spawn((
                OrderViz {
                    unit: entity,
                    kind: VizKind::Line {
                        order: order.clone(),
                    },
                },
                Mesh3d(assets.segment.clone()),
                MeshMaterial3d(assets.order_material[slot].clone()),
                segment_transform(origin, goal, LINE_THICKNESS, LINE_Y),
            ));
        }
        if !has_marker {
            commands.spawn((
                OrderViz {
                    unit: entity,
                    kind: VizKind::Marker {
                        order: order.clone(),
                    },
                },
                Mesh3d(assets.marker.clone()),
                MeshMaterial3d(assets.order_material[slot].clone()),
                Transform::from_translation(goal.with_y(MARKER_Y))
                    .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
            ));
        }
    }
    // Fade flashes out; drop lines/markers whose unit is gone, deselected
    // or done. Flashes are snapshots: they outlive orders and units.
    for (viz_entity, mut viz, _, material) in viz.iter_mut() {
        match &mut viz.kind {
            VizKind::Flash { remaining } => {
                *remaining -= dt;
                if *remaining <= 0.0 {
                    commands.entity(viz_entity).despawn();
                } else if let Some(mut mat) = materials.get_mut(&material.0) {
                    mat.base_color
                        .set_alpha((*remaining / FLASH_TIME).clamp(0.0, 1.0) * 0.9);
                }
            }
            VizKind::Line { .. } | VizKind::Marker { .. } => {
                if !live.contains(&(viz.unit, kind_tag(&viz.kind))) {
                    commands.entity(viz_entity).despawn();
                }
            }
        }
    }
}

fn kind_tag(kind: &VizKind) -> u8 {
    match kind {
        VizKind::Line { .. } => 0,
        VizKind::Marker { .. } => 1,
        VizKind::Flash { .. } => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_span_their_endpoints() {
        let from = Vec3::new(-10.0, 0.0, 5.0);
        let to = Vec3::new(30.0, 0.0, -15.0);
        let transform = segment_transform(from, to, 0.2, 0.08);
        assert!((transform.translation - Vec3::new(10.0, 0.08, -5.0)).length() < 0.0001);
        assert!((transform.scale.z - from.distance(to)).abs() < 0.001);
        assert_eq!(transform.scale.x, 0.2);
    }

    #[test]
    fn orders_map_to_stable_colour_slots() {
        assert_eq!(
            order_slot(&UnitOrder::Move {
                destination: Vec3::ZERO
            }),
            0
        );
        assert_eq!(
            order_slot(&UnitOrder::AttackMove {
                destination: Vec3::ZERO
            }),
            1
        );
        assert_eq!(order_slot(&UnitOrder::HoldPosition), 2);
        assert_eq!(order_slot(&UnitOrder::Idle), 3);
        assert_eq!(
            order_slot(&UnitOrder::Patrol {
                points: Vec::new(),
                next: 0
            }),
            4
        );
        assert_eq!(
            order_slot(&UnitOrder::Guard {
                target: Entity::from_bits(9)
            }),
            5
        );
        assert_eq!(
            order_slot(&UnitOrder::Build {
                site: Entity::from_bits(9)
            }),
            6
        );
    }
}
