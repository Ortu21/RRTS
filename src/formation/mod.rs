use bevy::prelude::*;

pub fn formation_slots(count: usize, center: Vec3, spacing: f32) -> Vec<Vec3> {
    if count == 0 {
        return Vec::new();
    }
    let columns = (count as f32).sqrt().ceil() as usize;
    let mut slots: Vec<Vec3> = (0..count)
        .map(|index| {
            Vec3::new(
                (index % columns) as f32 * spacing,
                0.0,
                (index / columns) as f32 * spacing,
            )
        })
        .collect();
    // Center the actual occupied slots, including an incomplete final row.
    let centroid = slots.iter().copied().sum::<Vec3>() / count as f32;
    for slot in &mut slots {
        *slot += center - centroid;
    }
    slots
}

/// Deterministic grid fill of one map half for skirmish stress tests.
/// Unlike `formation_slots`, the layout scales to any count: spacing shrinks
/// to fit instead of overflowing the map. Team 0 takes the west half, team 1
/// is mirrored so front lines face each other across a narrow gap.
pub fn skirmish_slots(count: usize, team: usize) -> Vec<Vec3> {
    if count == 0 {
        return Vec::new();
    }
    let (x_lo, x_hi) = (-95.0, -8.0);
    let (z_lo, z_hi) = (-95.0, 95.0);
    let width = x_hi - x_lo;
    let height = z_hi - z_lo;
    let cols = ((count as f32 * width / height).sqrt().ceil() as usize).max(1);
    let rows = count.div_ceil(cols);
    let step_x = width / cols as f32;
    let step_z = height / rows as f32;
    (0..count)
        .map(|index| {
            let column = index % cols;
            let row = index / cols;
            let west_x = x_lo + (column as f32 + 0.5) * step_x;
            Vec3::new(
                if team == 0 { west_x } else { -west_x },
                0.0,
                z_lo + (row as f32 + 0.5) * step_z,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_single_unit() {
        let center = Vec3::new(12.0, 0.0, -9.0);
        assert!(formation_slots(0, center, 2.5).is_empty());
        assert_eq!(formation_slots(1, center, 2.5), vec![center]);
    }

    #[test]
    fn slots_are_counted_centered_separated_and_deterministic() {
        let center = Vec3::new(-13.0, 0.0, 21.0);
        for count in [2, 3, 7, 10, 16, 99, 100] {
            let slots = formation_slots(count, center, 2.5);
            assert_eq!(slots.len(), count);
            assert_eq!(slots, formation_slots(count, center, 2.5));
            let centroid = slots.iter().copied().sum::<Vec3>() / count as f32;
            assert!(centroid.distance(center) < 0.0001);
            for (index, slot) in slots.iter().enumerate() {
                assert_eq!(slot.y, center.y);
                for other in &slots[index + 1..] {
                    assert!(slot.distance(*other) >= 2.5 - 0.0001);
                }
            }
        }
    }

    #[test]
    fn skirmish_halves_scale_and_face_each_other() {
        for count in [1, 100, 1000, 10_000] {
            let west = skirmish_slots(count, 0);
            let east = skirmish_slots(count, 1);
            assert_eq!(west.len(), count);
            assert_eq!(east.len(), count);
            assert_eq!(west, skirmish_slots(count, 0));
            for slot in west.iter().chain(&east) {
                // Stays inside the map, clear of the middle walls.
                assert!(slot.x.abs() <= 95.0 && slot.z.abs() <= 95.0);
                assert!(slot.x.abs() >= 8.0 - 0.0001 || slot.z.abs() > 15.0 + 0.8);
            }
            let west_front = west.iter().map(|slot| slot.x).fold(f32::MIN, f32::max);
            let east_front = east.iter().map(|slot| slot.x).fold(f32::MAX, f32::min);
            assert!(west_front <= -8.0 + 0.0001 && east_front >= 8.0 - 0.0001);
            // Dense front lines start inside acquisition range of each other.
            if count >= 100 {
                assert!(east_front - west_front <= 30.0);
            }
        }
    }
}
