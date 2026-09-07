use crate::{
    combat::ChaseTargets,
    navigation::PlanPaths,
    orders::UnitOrder,
    units::{CollisionRadius, Unit},
};
use bevy::prelude::*;
use std::collections::HashMap;

pub struct SpatialPlugin;

impl Plugin for SpatialPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpatialGrid>()
            .init_resource::<AvoidanceConfig>()
            .configure_sets(Update, SpatialSystems.before(PlanPaths))
            .add_systems(Update, rebuild_spatial.in_set(SpatialSystems))
            .add_systems(Update, apply_avoidance.after(ChaseTargets));
    }
}

/// Canonical body radius shared by spawning and avoidance tuning.
pub const DEFAULT_UNIT_RADIUS: f32 = 0.55;

/// Playable half-extent for unit bodies, mirroring navigation clearance.
pub const MAP_BOUND: f32 = crate::navigation::HALF_SIZE - crate::navigation::UNIT_CLEARANCE;

/// Ordering anchor: the index is rebuilt before path planning and combat,
/// avoidance is applied after every movement has been integrated.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpatialSystems;

/// Uniform spatial hash rebuilt every frame. One structure serves both
/// unit avoidance and target acquisition, so neither needs an O(n^2) scan.
#[derive(Resource, Default)]
pub struct SpatialGrid {
    pub cell_size: f32,
    cells: HashMap<(i32, i32), Vec<SpatialEntry>>,
    positions: HashMap<Entity, Vec3>,
}

#[derive(Clone, Copy)]
pub struct SpatialEntry {
    pub entity: Entity,
    pub position: Vec3,
    pub radius: f32,
}

impl SpatialGrid {
    #[cfg(test)]
    pub fn new(cell_size: f32) -> Self {
        Self {
            cell_size,
            cells: HashMap::new(),
            positions: HashMap::new(),
        }
    }

    pub fn cell_of(position: Vec3, cell_size: f32) -> (i32, i32) {
        (
            (position.x / cell_size).floor() as i32,
            (position.z / cell_size).floor() as i32,
        )
    }

    pub fn clear(&mut self) {
        for bucket in self.cells.values_mut() {
            bucket.clear();
        }
        self.positions.clear();
    }

    pub fn insert(&mut self, entity: Entity, position: Vec3, radius: f32) {
        let cell = Self::cell_of(position, self.cell_size);
        self.cells.entry(cell).or_default().push(SpatialEntry {
            entity,
            position,
            radius,
        });
        self.positions.insert(entity, position);
    }

    /// Last indexed position of an entity, if still present.
    pub fn position(&self, entity: Entity) -> Option<Vec3> {
        self.positions.get(&entity).copied()
    }

    /// Number of indexed entities sharing the caller's cell: live crowd
    /// density for congestion-aware planning. Single hash lookup, no scan.
    pub fn bucket_count(&self, position: Vec2) -> usize {
        let cell = Self::cell_of(Vec3::new(position.x, 0.0, position.y), self.cell_size);
        self.cells.get(&cell).map_or(0, Vec::len)
    }

    /// Calls `visit` for every indexed entity within `radius` of `position`.
    /// Cell visit order is deterministic; the caller decides the selection.
    pub fn for_each_nearby(
        &self,
        position: Vec3,
        radius: f32,
        mut visit: impl FnMut(&SpatialEntry),
    ) {
        let min = Self::cell_of(position - Vec3::new(radius, 0.0, radius), self.cell_size);
        let max = Self::cell_of(position + Vec3::new(radius, 0.0, radius), self.cell_size);
        let radius_squared = radius * radius;
        for cz in min.1..=max.1 {
            for cx in min.0..=max.0 {
                if let Some(bucket) = self.cells.get(&(cx, cz)) {
                    for entry in bucket {
                        let offset = entry.position - position;
                        if offset.x * offset.x + offset.z * offset.z <= radius_squared {
                            visit(entry);
                        }
                    }
                }
            }
        }
    }

    /// Nearest indexed entity within `radius`, excluding `ignore`.
    /// Ties break on entity index, so the result is deterministic.
    #[cfg(test)]
    pub fn nearest(&self, position: Vec3, radius: f32, ignore: Entity) -> Option<(Entity, f32)> {
        let mut best: Option<(Entity, f32)> = None;
        self.for_each_nearby(position, radius, |entry| {
            let entity = entry.entity;
            let entry_position = entry.position;
            if entity == ignore {
                return;
            }
            let distance_squared = entry_position.distance_squared(position);
            let replace = match best {
                None => true,
                Some((best_entity, best_distance)) => {
                    distance_squared < best_distance
                        || (distance_squared == best_distance
                            && entity.to_bits() < best_entity.to_bits())
                }
            };
            if replace {
                best = Some((entity, distance_squared));
            }
        });
        best.map(|(entity, distance_squared)| (entity, distance_squared.sqrt()))
    }
}

/// Centralized avoidance tuning. No magic numbers in the systems.
#[derive(Resource)]
pub struct AvoidanceConfig {
    pub unit_radius: f32,
    pub separation_radius: f32,
    pub separation_strength: f32,
    pub max_push: f32,
}

impl Default for AvoidanceConfig {
    fn default() -> Self {
        Self {
            unit_radius: DEFAULT_UNIT_RADIUS,
            separation_radius: 1.0,
            separation_strength: 4.0,
            max_push: 5.0,
        }
    }
}

/// Soft per-neighbor push on the XZ plane: full strength at zero distance,
/// fading linearly to zero at `separation_radius`.
pub fn separation_push(offset_xz: Vec2, separation_radius: f32, strength: f32) -> Vec2 {
    let distance = offset_xz.length();
    if distance >= separation_radius || distance <= f32::EPSILON {
        return Vec2::ZERO;
    }
    offset_xz / distance * (1.0 - distance / separation_radius) * strength
}

#[allow(clippy::type_complexity)]
fn rebuild_spatial(
    mut grid: ResMut<SpatialGrid>,
    config: Res<AvoidanceConfig>,
    units: Query<(Entity, &Transform), Or<(With<Unit>, With<crate::structures::Building>)>>,
    radii: Query<&CollisionRadius>,
) {
    if grid.cell_size <= 0.0 {
        grid.cell_size = config.separation_radius.max(2.0) * 4.0;
    }
    grid.clear();
    for (entity, transform) in &units {
        let radius = radii
            .get(entity)
            .map(|radius| radius.0)
            .unwrap_or(config.unit_radius);
        grid.insert(entity, transform.translation, radius);
    }
}

/// Local separation from spatial-grid neighbors only. Applied as a small
/// frame-rate independent displacement after movement, so strategic
/// destinations and formations are preserved.
/// Units holding position are avoidance anchors: the order promises they
/// stay, so crowds deflect around them instead of dragging them off post.
/// General for every locomotion kind (tanks and tank-like bipeds alike);
/// idle units keep the old soft behaviour and can still be nudged.
pub fn apply_avoidance(
    time: Res<Time>,
    grid: Res<SpatialGrid>,
    config: Res<AvoidanceConfig>,
    mut units: Query<(Entity, &mut Transform, &CollisionRadius, Option<&UnitOrder>), With<Unit>>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    // Read-only shared state copied out so the parallel closure stays
    // `Clone + Send + Sync`: each entity only writes its own Transform.
    let grid_ref: &SpatialGrid = &grid;
    let separation_radius = config.separation_radius;
    let unit_radius = config.unit_radius;
    let separation_strength = config.separation_strength;
    let max_push = config.max_push;
    units
        .par_iter_mut()
        .for_each(|(entity, mut transform, own_radius, order)| {
            // Holders stand ground: still solid for everybody else, but the
            // displacement is never applied to them.
            if matches!(order, Some(UnitOrder::HoldPosition)) {
                return;
            }
            let mut push = Vec2::ZERO;
            let position = transform.translation;
            // Lookup covers the widest possible trigger for standard bodies.
            let lookup = separation_radius + own_radius.0 + unit_radius;
            grid_ref.for_each_nearby(position, lookup, |other| {
                if other.entity == entity {
                    return;
                }
                // Bodies separate on contact distance plus a soft skin.
                let trigger = separation_radius + own_radius.0 + other.radius;
                let offset = (position - other.position).xz();
                push += separation_push(offset, trigger, separation_strength);
            });
            if push.length() > max_push {
                push = push.normalize_or_zero() * max_push;
            }
            transform.translation.x += push.x * dt;
            transform.translation.z += push.y * dt;
            // Never shove units out of the playable area: out-of-bounds
            // positions fail pathfinding and strand units without routes.
            transform.translation.x = transform.translation.x.clamp(-MAP_BOUND, MAP_BOUND);
            transform.translation.z = transform.translation.z.clamp(-MAP_BOUND, MAP_BOUND);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(index: u32) -> Entity {
        // All-zero bits are not a valid entity; shift test ids up by one.
        Entity::from_bits(index as u64 + 1)
    }

    #[test]
    fn cells_follow_position_and_size() {
        assert_eq!(SpatialGrid::cell_of(Vec3::ZERO, 8.0), (0, 0));
        assert_eq!(
            SpatialGrid::cell_of(Vec3::new(8.0, 0.0, -0.5), 8.0),
            (1, -1)
        );
        assert_eq!(
            SpatialGrid::cell_of(Vec3::new(-0.1, 0.0, 0.0), 8.0),
            (-1, 0)
        );
    }

    #[test]
    fn nearby_lookup_ignores_distant_entities_without_global_scan() {
        let mut grid = SpatialGrid::new(8.0);
        grid.insert(entity(1), Vec3::new(0.0, 0.0, 0.0), 0.55);
        grid.insert(entity(2), Vec3::new(3.0, 0.0, 0.0), 0.55);
        grid.insert(entity(3), Vec3::new(60.0, 0.0, 0.0), 0.55);
        let mut found = Vec::new();
        grid.for_each_nearby(Vec3::ZERO, 4.0, |entry| found.push(entry.entity));
        assert!(found.contains(&entity(1)));
        assert!(found.contains(&entity(2)));
        assert!(!found.contains(&entity(3)));
        assert_eq!(
            grid.nearest(Vec3::ZERO, 4.0, entity(1)),
            Some((entity(2), 3.0))
        );
        assert_eq!(
            grid.nearest(Vec3::ZERO, 4.0, entity(0)),
            Some((entity(1), 0.0))
        );
        assert!(grid.nearest(Vec3::ZERO, 1.0, entity(1)).is_none());
    }

    #[test]
    fn separation_is_soft_directional_and_bounded() {
        let radius = 2.0;
        assert_eq!(
            separation_push(Vec2::new(5.0, 0.0), radius, 4.0),
            Vec2::ZERO
        );
        assert_eq!(separation_push(Vec2::ZERO, radius, 4.0), Vec2::ZERO);
        let push = separation_push(Vec2::new(1.0, 0.0), radius, 4.0);
        assert!((push - Vec2::new(2.0, 0.0)).length() < 0.0001);
        // Closer neighbors push harder, always away from the neighbor.
        let close = separation_push(Vec2::new(-0.5, 0.0), radius, 4.0);
        assert!(close.x < 0.0 && close.length() > push.length());
    }
}
