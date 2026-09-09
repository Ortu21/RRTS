//! Picking matematico: ray→ground e ray→box, niente stato.
//!
//! Usato da `selection` (click/box) e `orders` (destinazioni). Gated su fog
//! dai chiamanti, non qui.

use bevy::prelude::*;

use crate::world::GROUND_HALF_SIZE;

pub fn ground_position(camera: &Camera, transform: &GlobalTransform, cursor: Vec2) -> Option<Vec3> {
    let ray = camera.viewport_to_world(transform, cursor).ok()?;
    let point = ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))?;
    (point.x.abs() <= GROUND_HALF_SIZE && point.z.abs() <= GROUND_HALF_SIZE).then_some(point)
}

// Slab intersection against the axis-aligned primitive used for units. Nearest hit wins.
pub fn ray_box_distance(ray: &Ray3d, center: Vec3, half_size: Vec3) -> Option<f32> {
    let min = center - half_size;
    let max = center + half_size;
    let mut near = 0.0_f32;
    let mut far = f32::INFINITY;
    for axis in 0..3 {
        let origin = ray.origin[axis];
        let direction = ray.direction[axis];
        if direction.abs() < 1e-6 {
            if origin < min[axis] || origin > max[axis] {
                return None;
            }
        } else {
            let first = (min[axis] - origin) / direction;
            let second = (max[axis] - origin) / direction;
            near = near.max(first.min(second));
            far = far.min(first.max(second));
            if near > far {
                return None;
            }
        }
    }
    Some(near)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_hits_only_boxes_in_front_including_parallel_boundary() {
        let ray = Ray3d::new(Vec3::new(0.0, 0.0, 5.0), Dir3::NEG_Z);
        assert_eq!(ray_box_distance(&ray, Vec3::ZERO, Vec3::ONE), Some(4.0));
        assert_eq!(ray_box_distance(&ray, Vec3::X, Vec3::ONE), Some(4.0));
        assert_eq!(ray_box_distance(&ray, Vec3::X * 3.0, Vec3::ONE), None);
        assert_eq!(ray_box_distance(&ray, Vec3::Z * 10.0, Vec3::ONE), None);
    }
}
